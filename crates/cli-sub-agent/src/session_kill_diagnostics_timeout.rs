#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimeoutDiagnostics {
    pub(crate) requested_timeout_seconds: Option<u64>,
    pub(crate) idle_timeout_seconds: Option<u64>,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
}

impl TimeoutDiagnostics {
    pub(crate) fn from_execution_options(
        requested_timeout_seconds: Option<u64>,
        idle_timeout_seconds: u64,
        initial_response_timeout_seconds: Option<u64>,
    ) -> Self {
        Self {
            requested_timeout_seconds,
            idle_timeout_seconds: (idle_timeout_seconds != u64::MAX)
                .then_some(idle_timeout_seconds),
            initial_response_timeout_seconds,
        }
    }

    pub(crate) fn detail_parts(&self, terminal_reason: Option<&str>) -> Vec<String> {
        let mut details = Vec::new();
        if let Some(seconds) = self.requested_timeout_seconds {
            details.push(format!("requested_timeout_seconds={seconds}"));
        }
        let kind = match terminal_reason {
            Some("initial_response_timeout") => "initial_response_timeout",
            Some("idle_timeout") => "idle_timeout",
            Some("timeout") => "wall_timeout",
            _ => "timeout",
        };
        details.push(format!("effective_timeout_kind={kind}"));
        match kind {
            "initial_response_timeout" => self.push_effective(
                &mut details,
                self.initial_response_timeout_seconds,
                "initial_response_timeout",
            ),
            "idle_timeout" => {
                self.push_effective(&mut details, self.idle_timeout_seconds, "idle_timeout")
            }
            "wall_timeout" => {
                push_timeout_field(
                    &mut details,
                    "effective_timeout_seconds",
                    self.requested_timeout_seconds,
                );
                details.push("effective_timeout_source=wall_timeout".to_string());
            }
            _ => {}
        }
        push_timeout_field(
            &mut details,
            "idle_timeout_seconds",
            self.idle_timeout_seconds,
        );
        push_timeout_field(
            &mut details,
            "initial_response_timeout_seconds",
            self.initial_response_timeout_seconds,
        );
        details
    }

    fn push_effective(
        &self,
        details: &mut Vec<String>,
        effective: Option<u64>,
        source: &'static str,
    ) {
        push_timeout_field(details, "effective_timeout_seconds", effective);
        details.push(format!(
            "effective_timeout_source={}",
            timeout_source(self.requested_timeout_seconds, effective, source)
        ));
    }
}

fn push_timeout_field(parts: &mut Vec<String>, name: &str, seconds: Option<u64>) {
    match seconds {
        Some(seconds) => parts.push(format!("{name}={seconds}")),
        None => parts.push(format!("{name}=disabled")),
    }
}

fn timeout_source(
    requested_timeout_seconds: Option<u64>,
    effective_timeout_seconds: Option<u64>,
    fallback_source: &'static str,
) -> &'static str {
    match (requested_timeout_seconds, effective_timeout_seconds) {
        (Some(requested), Some(effective)) if effective >= requested => "wall_timeout_floor",
        _ => fallback_source,
    }
}
