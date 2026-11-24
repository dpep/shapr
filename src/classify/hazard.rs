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
    /// An argument restating a default dropped or added: the hazard is in
    /// the called method's own body.
    RestateDefault,
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
            | Binding | Frame | Line => true,
            File => false,
        },
        // Code reading the frame (`binding`, `$1`) sees the statements'
        // locals and matches leave, or arrive.
        Op::ExtractCaller => match hazard {
            Binding | Frame => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Line | File => false,
        },
        // `super` reaches the parent's method of the new name; `__method__`
        // reads the new name.
        Op::RenameMethod => match hazard {
            Super | Frame => true,
            Return | Break | Next | Redo | Retry | Yield | BlockGiven | Await | Macro | Binding
            | Line | File => false,
        },
        // `binding` and `eval` read locals by name.
        Op::RenameLocal => match hazard {
            Binding => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Frame | Line | File => false,
        },
        Op::Move => match hazard {
            Line => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Binding | Frame | File => false,
        },
        Op::MoveAcrossFiles => match hazard {
            Line | File => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Binding | Frame => false,
        },
        // The parameter is bound either way; a method reading its own frame
        // is left alone rather than reasoned about.
        Op::RestateDefault => match hazard {
            Binding | Frame => true,
            Return | Break | Next | Redo | Retry | Yield | Super | BlockGiven | Await | Macro
            | Line | File => false,
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
        assert!(blocks(Op::RestateDefault, Hazard::Binding));
        assert!(!blocks(Op::RestateDefault, Hazard::Super));
    }
}
