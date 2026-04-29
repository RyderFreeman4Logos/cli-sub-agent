const PERSISTENT_RATE_LIMIT_THRESHOLD: u8 = 3;

#[derive(Debug, Default)]
pub(crate) struct PersistentRateLimitTracker {
    last_message: Option<String>,
    consecutive_count: u8,
}

impl PersistentRateLimitTracker {
    pub(crate) fn observe_appended_output(&mut self, text: &str) -> Option<String> {
        for line in text.lines() {
            let Some(message) = normalize_persistent_rate_limit_line(line) else {
                self.last_message = None;
                self.consecutive_count = 0;
                continue;
            };

            if self.last_message.as_deref() == Some(message.as_str()) {
                self.consecutive_count = self.consecutive_count.saturating_add(1);
            } else {
                self.last_message = Some(message.clone());
                self.consecutive_count = 1;
            }

            if self.consecutive_count >= PERSISTENT_RATE_LIMIT_THRESHOLD {
                return Some(format!(
                    "429_quota_exhausted: repeated {} identical 429/quota errors: {message}",
                    self.consecutive_count
                ));
            }
        }
        None
    }
}

fn normalize_persistent_rate_limit_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    let is_rate_limit = lowered.contains("429")
        || lowered.contains("too many requests")
        || lowered.contains("rate limit")
        || (lowered.contains("quota")
            && (lowered.contains("exceed")
                || lowered.contains("exhaust")
                || lowered.contains("resource exhausted")));
    is_rate_limit.then(|| trimmed.split_whitespace().collect::<Vec<_>>().join(" "))
}
