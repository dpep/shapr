            for statement in statements(line) {
                // A local is the method's: another method's of the same name
                // is another local.
                if statement.trim_start().starts_with("def ") {
                    held.clear();
                }
                hold(&mut held, statement, indent);
                for (shape, to) in sent_strings(statement, &held) {
                    sendings.push(Sending {
                        shape,
                        to,
                        nesting: None,
                        at: at.clone(),
                    });
                }
            }
