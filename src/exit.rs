use std::io;

use crate::prompts::get_prompts;

#[derive(Debug)]
pub enum AppError {
    Cancelled,
    Failure(String),
}

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::Cancelled => 0,
            AppError::Failure(_) => 1,
        }
    }
}

pub fn exit_failure(message: impl Into<String>) -> AppError {
    AppError::Failure(message.into())
}

/// Map interrupt/cancel from a prompt result into `AppError::Cancelled` after cancel feedback.
pub fn handle_prompt_cancel(err: &io::Error) -> Option<AppError> {
    if err.kind() == io::ErrorKind::Interrupted {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        Some(AppError::Cancelled)
    } else {
        None
    }
}

pub fn map_prompt_result<T>(result: io::Result<T>) -> Result<T, AppError> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            if let Some(cancel) = handle_prompt_cancel(&err) {
                Err(cancel)
            } else {
                Err(AppError::Failure(err.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::{create_scripted_prompts, set_prompts_for_tests};
    use serde_json::json;

    #[test]
    fn failure_exit_code_is_nonzero() {
        assert_eq!(exit_failure("boom").exit_code(), 1);
    }

    #[test]
    fn cancel_exit_code_is_zero() {
        assert_eq!(AppError::Cancelled.exit_code(), 0);
    }

    #[test]
    fn map_prompt_result_maps_interrupt_to_cancelled() {
        set_prompts_for_tests(Some(create_scripted_prompts(vec![json!("x")])));
        let result = map_prompt_result::<()>(Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "cancel",
        )));
        set_prompts_for_tests(None);
        assert!(matches!(result, Err(AppError::Cancelled)));
    }

    #[test]
    fn map_prompt_result_passes_through_ok() {
        assert_eq!(map_prompt_result(Ok(42)).unwrap(), 42);
    }
}
