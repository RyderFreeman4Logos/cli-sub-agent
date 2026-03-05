use std::sync::{LazyLock, Mutex};

pub(crate) static TEST_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
