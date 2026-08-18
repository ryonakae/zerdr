use std::env;

use crate::error::{Error, Result};

pub fn require_zed_terminal() -> Result<()> {
    let in_zed = env::var("ZED_TERM").is_ok_and(|value| value == "true")
        && env::var("TERM_PROGRAM").is_ok_and(|value| value == "zed");

    if in_zed {
        Ok(())
    } else {
        Err(Error::User(
            "this command must run in a Zed integrated terminal (ZED_TERM=true and TERM_PROGRAM=zed)"
                .to_owned(),
        ))
    }
}
