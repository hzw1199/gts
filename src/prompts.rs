use std::io;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct InputOpts {
    pub message: String,
    pub placeholder: Option<String>,
    pub default_input: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConfirmOpts {
    pub message: String,
    pub initial_value: bool,
}

#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub hint: String,
}

#[derive(Debug, Clone)]
pub struct SelectOpts {
    pub message: String,
    pub items: Vec<SelectItem>,
    pub initial_value: Option<String>,
}

pub trait PromptsApi: Send + Sync {
    fn intro(&self, title: &str) -> io::Result<()>;
    fn outro(&self, message: &str) -> io::Result<()>;
    fn outro_cancel(&self, message: &str) -> io::Result<()>;
    fn note(&self, prompt: &str, message: &str) -> io::Result<()>;
    fn input(&self, opts: InputOpts) -> io::Result<String>;
    fn confirm(&self, opts: ConfirmOpts) -> io::Result<bool>;
    fn select(&self, opts: SelectOpts) -> io::Result<String>;
    fn spinner_start(&self, message: &str) -> io::Result<()>;
    fn spinner_stop(&self, message: &str) -> io::Result<()>;
}

struct CliclackPrompts;

impl PromptsApi for CliclackPrompts {
    fn intro(&self, title: &str) -> io::Result<()> {
        cliclack::intro(title)
    }

    fn outro(&self, message: &str) -> io::Result<()> {
        cliclack::outro(message)
    }

    fn outro_cancel(&self, message: &str) -> io::Result<()> {
        cliclack::outro_cancel(message)
    }

    fn note(&self, prompt: &str, message: &str) -> io::Result<()> {
        cliclack::note(prompt, message)
    }

    fn input(&self, opts: InputOpts) -> io::Result<String> {
        let mut prompt = cliclack::input(opts.message);
        if let Some(placeholder) = &opts.placeholder {
            prompt = prompt.placeholder(placeholder);
        }
        if let Some(default_input) = &opts.default_input {
            prompt = prompt.default_input(default_input);
        }
        prompt
            .validate(|v: &String| {
                if v.trim().is_empty() {
                    Err("Value is required")
                } else {
                    Ok(())
                }
            })
            .interact()
    }

    fn confirm(&self, opts: ConfirmOpts) -> io::Result<bool> {
        cliclack::confirm(opts.message)
            .initial_value(opts.initial_value)
            .interact()
    }

    fn select(&self, opts: SelectOpts) -> io::Result<String> {
        let mut prompt = cliclack::select(opts.message);
        for item in &opts.items {
            prompt = prompt.item(item.value.clone(), &item.label, &item.hint);
        }
        if let Some(initial) = opts.initial_value {
            prompt = prompt.initial_value(initial);
        }
        prompt.interact()
    }

    fn spinner_start(&self, message: &str) -> io::Result<()> {
        let spinner = cliclack::spinner();
        spinner.start(message);
        Ok(())
    }

    fn spinner_stop(&self, message: &str) -> io::Result<()> {
        let spinner = cliclack::spinner();
        spinner.stop(message);
        Ok(())
    }
}

struct ScriptedPrompts {
    answers: Mutex<Vec<serde_json::Value>>,
}

impl ScriptedPrompts {
    fn from_json_array(raw: &str) -> Result<Self, String> {
        let answers: Vec<serde_json::Value> = serde_json::from_str(raw)
            .map_err(|_| "Invalid GTS_PROMPT_SCRIPT: must be a JSON array".to_string())?;
        Ok(Self {
            answers: Mutex::new(answers),
        })
    }

    fn next(&self) -> io::Result<serde_json::Value> {
        let mut answers = self.answers.lock().unwrap();
        if answers.is_empty() {
            return Err(io::Error::other("Ran out of scripted prompt answers"));
        }
        Ok(answers.remove(0))
    }
}

impl PromptsApi for ScriptedPrompts {
    fn intro(&self, title: &str) -> io::Result<()> {
        if !title.is_empty() {
            println!("{title}");
        }
        Ok(())
    }

    fn outro(&self, message: &str) -> io::Result<()> {
        if !message.is_empty() {
            println!("{message}");
        }
        Ok(())
    }

    fn outro_cancel(&self, message: &str) -> io::Result<()> {
        let _ = cliclack::outro_cancel(message);
        Ok(())
    }

    fn note(&self, prompt: &str, message: &str) -> io::Result<()> {
        println!("{prompt}: {message}");
        Ok(())
    }

    fn input(&self, _opts: InputOpts) -> io::Result<String> {
        let value = self.next()?;
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| io::Error::other("Expected string answer for input"))
    }

    fn confirm(&self, _opts: ConfirmOpts) -> io::Result<bool> {
        let value = self.next()?;
        value
            .as_bool()
            .ok_or_else(|| io::Error::other("Expected bool answer for confirm"))
    }

    fn select(&self, _opts: SelectOpts) -> io::Result<String> {
        let value = self.next()?;
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| io::Error::other("Expected string answer for select"))
    }

    fn spinner_start(&self, _message: &str) -> io::Result<()> {
        Ok(())
    }

    fn spinner_stop(&self, _message: &str) -> io::Result<()> {
        Ok(())
    }
}

static OVERRIDE: OnceLock<Mutex<Option<Arc<dyn PromptsApi>>>> = OnceLock::new();
static SCRIPTED_FROM_ENV: OnceLock<Option<Arc<dyn PromptsApi>>> = OnceLock::new();

fn override_slot() -> &'static Mutex<Option<Arc<dyn PromptsApi>>> {
    OVERRIDE.get_or_init(|| Mutex::new(None))
}

pub fn set_prompts_for_tests(next: Option<Arc<dyn PromptsApi>>) {
    *override_slot().lock().unwrap() = next;
}

fn scripted_from_env() -> Option<Arc<dyn PromptsApi>> {
    SCRIPTED_FROM_ENV
        .get_or_init(|| {
            let raw = std::env::var("GTS_PROMPT_SCRIPT").ok()?;
            if raw.is_empty() {
                return None;
            }
            Some(Arc::new(
                ScriptedPrompts::from_json_array(&raw).unwrap_or_else(|err| panic!("{err}")),
            ) as Arc<dyn PromptsApi>)
        })
        .clone()
}

pub fn get_prompts() -> Arc<dyn PromptsApi> {
    if let Some(overridden) = override_slot().lock().unwrap().clone() {
        return overridden;
    }
    if let Some(scripted) = scripted_from_env() {
        return scripted;
    }
    Arc::new(CliclackPrompts)
}

pub fn create_scripted_prompts(answers: Vec<serde_json::Value>) -> Arc<dyn PromptsApi> {
    Arc::new(ScriptedPrompts {
        answers: Mutex::new(answers),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scripted_input_returns_injected_answers() {
        let prompts = create_scripted_prompts(vec![json!("alpha")]);
        let value = prompts
            .input(InputOpts {
                message: "Enter a name".into(),
                placeholder: Some("example".into()),
                default_input: None,
            })
            .unwrap();
        assert_eq!(value, "alpha");
    }

    #[test]
    fn scripted_confirm_and_select() {
        let prompts = create_scripted_prompts(vec![json!(true), json!("feature")]);
        assert!(prompts
            .confirm(ConfirmOpts {
                message: "Continue?".into(),
                initial_value: false,
            })
            .unwrap());
        assert_eq!(
            prompts
                .select(SelectOpts {
                    message: "Pick".into(),
                    items: vec![SelectItem {
                        value: "feature".into(),
                        label: "feature".into(),
                        hint: String::new(),
                    }],
                    initial_value: None,
                })
                .unwrap(),
            "feature"
        );
    }
}
