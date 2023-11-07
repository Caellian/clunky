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
