//! Prompt assembly helpers for experimental KV-cache-friendly ordering.

use crate::pipeline::design_context::FirstTurnContext;

pub(crate) const STATIC_START: &str = "<!-- CSA:CACHE_BOUNDARY:STATIC_START -->";
pub(crate) const STATIC_END: &str = "<!-- CSA:CACHE_BOUNDARY:STATIC_END -->";

#[derive(Debug)]
pub(crate) struct PromptAssembly {
    enable_prompt_caching: bool,
    static_sections: Vec<String>,
    dynamic_prompt: String,
}

impl PromptAssembly {
    pub(crate) fn new(dynamic_prompt: String, enable_prompt_caching: bool) -> Self {
        Self {
            enable_prompt_caching,
            static_sections: Vec::new(),
            dynamic_prompt,
        }
    }

    pub(crate) fn prepend_dynamic(&mut self, prefix: &str) {
        self.dynamic_prompt = format!("{prefix}{}", self.dynamic_prompt);
    }

    pub(crate) fn add_first_turn_context(&mut self, context: FirstTurnContext) {
        if let Some(project_context) = context.project_context {
            if self.enable_prompt_caching {
                self.static_sections.push(project_context);
            } else {
                self.dynamic_prompt = format!("{project_context}{}", self.dynamic_prompt);
            }
        }

        if let Some(design_context) = context.design_context {
            if !self.dynamic_prompt.ends_with('\n') {
                self.dynamic_prompt.push('\n');
            }
            self.dynamic_prompt.push_str(&design_context);
        }
    }

    pub(crate) fn dynamic_prompt_mut(&mut self) -> &mut String {
        &mut self.dynamic_prompt
    }

    pub(crate) fn add_restriction_instructions(&mut self, instructions: Option<&str>) {
        let Some(instructions) = instructions else {
            return;
        };
        if self.enable_prompt_caching {
            self.static_sections.push(instructions.to_string());
        } else {
            self.dynamic_prompt = format!("{instructions}\n\n{}", self.dynamic_prompt);
        }
    }

    pub(crate) fn append_dynamic_block(&mut self, block: &str) {
        self.dynamic_prompt = format!("{}\n\n{block}", self.dynamic_prompt);
    }

    pub(crate) fn add_static_or_append_dynamic(&mut self, section: &str) {
        if self.enable_prompt_caching {
            self.static_sections.push(section.to_string());
        } else {
            self.dynamic_prompt.push_str(section);
        }
    }

    pub(crate) fn finish(self) -> String {
        if !self.enable_prompt_caching || self.static_sections.is_empty() {
            return self.dynamic_prompt;
        }

        let mut output = String::new();
        output.push_str(STATIC_START);
        output.push('\n');
        for section in self.static_sections {
            if section.is_empty() {
                continue;
            }
            output.push_str(&section);
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push('\n');
        }
        output.push_str(STATIC_END);
        output.push('\n');
        output.push_str(&self.dynamic_prompt);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_turn_context() -> FirstTurnContext {
        FirstTurnContext {
            project_context: Some(
                "<context-file path=\"CLAUDE.md\">\nstatic\n</context-file>\n\n".to_string(),
            ),
            design_context: Some("<design-context>\ndynamic design\n</design-context>".to_string()),
        }
    }

    #[test]
    fn disabled_preserves_legacy_order_without_markers() {
        let mut assembly = PromptAssembly::new("user task".to_string(), false);
        assembly.add_first_turn_context(first_turn_context());
        assembly
            .dynamic_prompt_mut()
            .push_str("\n<memory>dynamic</memory>");
        assembly.add_restriction_instructions(Some("STATIC RESTRICTION"));
        assembly.add_static_or_append_dynamic("\n\n<csa-output-format>static</csa-output-format>");

        let prompt = assembly.finish();

        assert!(!prompt.contains(STATIC_START));
        assert!(!prompt.contains(STATIC_END));
        assert!(prompt.starts_with("STATIC RESTRICTION\n\n<context-file"));
        assert!(prompt.contains("user task\n<design-context>"));
        assert!(prompt.ends_with("<csa-output-format>static</csa-output-format>"));
    }

    #[test]
    fn enabled_groups_static_block_before_dynamic_prompt() {
        let mut assembly = PromptAssembly::new("user task".to_string(), true);
        assembly.prepend_dynamic("dynamic warning\n");
        assembly.add_first_turn_context(first_turn_context());
        assembly
            .dynamic_prompt_mut()
            .push_str("\n<memory>dynamic</memory>");
        assembly.add_restriction_instructions(Some("STATIC RESTRICTION"));
        assembly.add_static_or_append_dynamic("\n\n<csa-output-format>static</csa-output-format>");
        assembly.append_dynamic_block("<guard>dynamic guard</guard>");

        let prompt = assembly.finish();

        let static_start = prompt.find(STATIC_START).expect("static start marker");
        let context = prompt.find("<context-file").expect("static context");
        let restriction = prompt.find("STATIC RESTRICTION").expect("restriction");
        let output_format = prompt.find("<csa-output-format>").expect("output format");
        let static_end = prompt.find(STATIC_END).expect("static end marker");
        let dynamic_warning = prompt.find("dynamic warning").expect("dynamic warning");
        let user_task = prompt.find("user task").expect("user task");
        let memory = prompt.find("<memory>dynamic</memory>").expect("memory");
        let guard = prompt.find("<guard>dynamic guard</guard>").expect("guard");

        assert_eq!(static_start, 0);
        assert!(static_start < context);
        assert!(context < restriction);
        assert!(restriction < output_format);
        assert!(output_format < static_end);
        assert!(static_end < dynamic_warning);
        assert!(dynamic_warning < user_task);
        assert!(user_task < memory);
        assert!(memory < guard);
    }
}
