use super::Executor;

impl Executor {
    pub fn restriction_instructions(
        &self,
        allow_edit: bool,
        allow_write_new: bool,
    ) -> Option<&'static str> {
        if !allow_edit && !allow_write_new {
            Some(
                "IMPORTANT RESTRICTION: You are in READ-ONLY mode. \
                 You MUST NOT edit existing files, create new files, or run commands \
                 that modify the filesystem. You may ONLY perform read-only analysis, \
                 search, and reporting.",
            )
        } else if !allow_edit {
            Some(
                "IMPORTANT RESTRICTION: You MUST NOT edit or modify any existing files. \
                 You may only create new files or perform read-only analysis.",
            )
        } else if !allow_write_new {
            Some(
                "IMPORTANT RESTRICTION: You MUST NOT create new files. \
                 You may only edit existing files or perform read-only analysis.",
            )
        } else {
            None
        }
    }

    /// Apply restrictions by modifying the prompt to include restriction instructions.
    /// Returns the modified prompt.
    ///
    /// `allow_edit`: when false, tool must not modify existing files.
    /// `allow_write_new`: when false, tool must not create new files either.
    pub fn apply_restrictions(
        &self,
        prompt: &str,
        allow_edit: bool,
        allow_write_new: bool,
    ) -> String {
        if let Some(instructions) = self.restriction_instructions(allow_edit, allow_write_new) {
            format!("{instructions}\n\n{prompt}")
        } else {
            prompt.to_string()
        }
    }
}
