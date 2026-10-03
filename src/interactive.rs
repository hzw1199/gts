use std::io::{self, IsTerminal};

#[derive(Debug, Clone, Default)]
pub struct InteractiveOptions {
    pub no_interactive: bool,
    pub ci: Option<String>,
    pub is_tty: Option<bool>,
    pub force_interactive: Option<bool>,
}

pub fn is_interactive(opts: InteractiveOptions) -> bool {
    if opts.no_interactive {
        return false;
    }
    let ci = opts.ci.or_else(|| std::env::var("CI").ok());
    if ci.as_deref().is_some_and(|v| !v.is_empty()) {
        return false;
    }
    let force = opts.force_interactive.unwrap_or_else(|| {
        std::env::var("GTS_FORCE_INTERACTIVE").ok().as_deref() == Some("1")
    });
    if force {
        return true;
    }
    opts.is_tty
        .unwrap_or_else(|| io::stdin().is_terminal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn true_when_tty_no_ci_interactive_allowed() {
        assert!(is_interactive(InteractiveOptions {
            is_tty: Some(true),
            ci: None,
            no_interactive: false,
            force_interactive: Some(false),
        }));
    }

    #[test]
    fn false_when_not_tty() {
        assert!(!is_interactive(InteractiveOptions {
            is_tty: Some(false),
            ci: None,
            no_interactive: false,
            force_interactive: Some(false),
        }));
    }

    #[test]
    fn false_when_ci_set() {
        assert!(!is_interactive(InteractiveOptions {
            is_tty: Some(true),
            ci: Some("true".into()),
            no_interactive: false,
            force_interactive: Some(false),
        }));
    }

    #[test]
    fn false_when_no_interactive() {
        assert!(!is_interactive(InteractiveOptions {
            is_tty: Some(true),
            ci: None,
            no_interactive: true,
            force_interactive: Some(false),
        }));
    }

    #[test]
    fn true_when_force_interactive_without_tty() {
        assert!(is_interactive(InteractiveOptions {
            is_tty: Some(false),
            ci: None,
            no_interactive: false,
            force_interactive: Some(true),
        }));
    }
}
