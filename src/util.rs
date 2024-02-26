use std::process::exit;

#[macro_export]
macro_rules! require_some {
    ($value: expr) => {
        require_some!(($value) or return)
    };
    (($value: expr) or return) => {
        match $value {
            Some(it) => it,
            None => return
        }
    };
    (($value: expr) or return $else: expr) => {
        match $value {
            Some(it) => it,
            None => return $else
        }
    };
    (($value: expr) or break) => {
        match $value {
            Some(it) => it,
            None => break,
        }
    };
    (($value: expr) or break $else: expr) => {
        match $value {
            Some(it) => it,
            None => break $else,
        }
    };
}

pub trait ErrHandleExt<T> {
    fn some_or_log(self, details: Option<String>) -> Option<T>;
    fn or_trace(self, details: Option<String>, code: i32) -> T;
}
impl<T, E: std::error::Error> ErrHandleExt<T> for Result<T, E> {
    fn some_or_log(self, details: Option<String>) -> Option<T> {
        let error = match self {
            Ok(it) => return Some(it),
            Err(err) => err,
        };

        let details = details.map(|it| it + ": ").unwrap_or_default();
        let mut msg = format!("{}{}", details, error);
        let mut current = error.source();
        while let Some(err) = current {
            msg.push('\n');
            msg.push_str(&err.to_string());
            current = err.source();
        }
        log::error!("{}", msg);

        None
    }

    fn or_trace(self, details: Option<String>, code: i32) -> T {
        match self.some_or_log(details) {
            Some(it) => it,
            None => exit(code),
        }
    }
}
