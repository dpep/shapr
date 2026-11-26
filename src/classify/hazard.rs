//! Which hazards stop which operations: one table every detector consults.
//!
//! Neither match has a wildcard arm, so adding a [`Hazard`] or an [`Op`]
//! doesn't compile until every pairing is decided.

use crate::tree::Hazard;

/// A structural operation a detector proves, by the code a hazard would sit
/// in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    /// Statements moved into a new method (an extract), or a method's body
    /// put in place of its call (an inline): the hazard is in them.
    Extract,
    /// The method those statements leave or join: the hazard is anywhere in
    /// it.
    ExtractCaller,
    /// A method renamed: the hazard is in its own body.
    RenameMethod,
    /// A local renamed: the hazard is anywhere in its scope.
    RenameLocal,
    /// A declaration moved within its file: the hazard is in it.
    Move,
    /// A declaration moved to another file.
    MoveAcrossFiles,
    /// Lines added or removed above the code, or the code's own line edited:
    /// it sits at another line or column.
    Shift,
}

/// Whether `hazard` in the code `op` touches makes it something other than
/// a structural change.
pub(super) fn blocks(op: Op, hazard: Hazard) -> bool {
    use Hazard::*;
    match op {
        // Control flow and frame reads bind to the method around them; the
        // statements stay in their file, so `__FILE__` reads the same.
        Op::Extract => match hazard {
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Binding | Frame | Stack | Line => true,
            File => false,
        },
        // Code reading the frame (`binding`, `$1`) sees the statements'
        // locals and matches leave, or arrive.
        Op::ExtractCaller => match hazard {
            Binding | Frame | Stack => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Line | File => false,
        },
        // `super` reaches the parent's method of the new name; `__method__`
        // reads the new name.
        Op::RenameMethod => match hazard {
            Super | Frame | Stack => true,
            Return | Break | Next | Redo | Retry | Yield | BlockGiven | Await | Macro | Binding
            | Line | File => false,
        },
        // `binding` and `eval` read locals by name.
        Op::RenameLocal => match hazard {
            Binding => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Frame | Stack | Line | File => false,
        },
        // A stack read reports its own line, and across files its file.
        Op::Move => match hazard {
            Line | Stack => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Binding | Frame | File => false,
        },
        Op::MoveAcrossFiles => match hazard {
            Line | File | Stack => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Binding | Frame => false,
        },
        // Only an explicit read of the code's own position or the stack's; a
        // backtrace reports shifted lines too, as any exception does.
        Op::Shift => match hazard {
            Line | Stack => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Binding | Frame | File => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_stops_on_its_own_hazards() {
        assert!(blocks(Op::Extract, Hazard::Return) && !blocks(Op::Extract, Hazard::File));
        assert!(blocks(Op::ExtractCaller, Hazard::Binding));
        assert!(!blocks(Op::ExtractCaller, Hazard::Return));
        assert!(blocks(Op::RenameMethod, Hazard::Super));
        assert!(!blocks(Op::RenameMethod, Hazard::Binding));
        assert!(blocks(Op::RenameLocal, Hazard::Binding));
        assert!(!blocks(Op::RenameLocal, Hazard::Frame));
        assert!(blocks(Op::Move, Hazard::Line) && !blocks(Op::Move, Hazard::File));
        assert!(blocks(Op::MoveAcrossFiles, Hazard::File));
        assert!(blocks(Op::Shift, Hazard::Line) && !blocks(Op::Shift, Hazard::Frame));
        assert!(blocks(Op::Shift, Hazard::Stack) && blocks(Op::RenameMethod, Hazard::Stack));
    }
}
