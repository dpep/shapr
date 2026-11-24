        conn.execute_batch(&format!("PRAGMA busy_timeout={};", BUSY_WAIT.as_millis()))?;
