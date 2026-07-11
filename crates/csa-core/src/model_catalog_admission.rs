impl EffectiveModelCatalog {

    pub fn validate_parts(
        &self,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
    ) -> Result<CatalogAdmission, CatalogLegalityError> {
        if let Some(field) = malformed_identity_field(tool, provider, model, reasoning) {
            return Err(Self::malformed_error(
                field,
                tool,
                provider,
                model,
                reasoning,
                self.policy_provenance.source_label(),
            ));
        }
        let identity = ModelIdentity {
            tool: tool.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
        };
        let configured_provenance = self.configured_provenance(&identity, reasoning);
        if let Some(capability) = self.entries.get(&identity) {
            if !capability.enabled {
                return Err(Self::error(
                    CatalogErrorKind::DisabledModel,
                    tool,
                    provider,
                    model,
                    reasoning,
                    Vec::new(),
                    capability.provenance.source_label(),
                ));
            }
            return match self.validate_capability(tool, provider, model, reasoning, capability) {
                Ok(admission) => Ok(admission),
                Err(error)
                    if configured_provenance.is_some()
                        && matches!(
                            error.kind(),
                            CatalogErrorKind::UnsupportedReasoningEffort
                                | CatalogErrorKind::UnsupportedCustomReasoning
                        ) =>
                {
                    Ok(Self::configured_admission(
                        tool,
                        provider,
                        model,
                        reasoning,
                        configured_provenance
                            .clone()
                            .expect("checked configured provenance"),
                        CatalogWarningKind::UnverifiedReasoningEffort,
                    ))
                }
                Err(error) => Err(error),
            };
        }

        if let Some(capability) = self.matching_scope(tool, provider) {
            return match self.validate_capability(tool, provider, model, reasoning, capability) {
                Ok(admission) => Ok(admission),
                Err(error)
                    if configured_provenance.is_some()
                        && matches!(
                            error.kind(),
                            CatalogErrorKind::UnsupportedReasoningEffort
                                | CatalogErrorKind::UnsupportedCustomReasoning
                        ) =>
                {
                    Ok(Self::configured_admission(
                        tool,
                        provider,
                        model,
                        reasoning,
                        configured_provenance
                            .clone()
                            .expect("checked configured provenance"),
                        CatalogWarningKind::UnverifiedReasoningEffort,
                    ))
                }
                Err(error) => Err(error),
            };
        }

        if let Some(provenance) = configured_provenance {
            return Ok(Self::configured_admission(
                tool,
                provider,
                model,
                reasoning,
                provenance,
                CatalogWarningKind::UnverifiedModel,
            ));
        }

        if !self.closed {
            return Ok(CatalogAdmission {
                provenance: self.policy_provenance.clone(),
                warning: None,
            });
        }

        let known_tools = self.known_tools();
        if !known_tools.iter().any(|known| known == tool) {
            return Err(Self::error(
                CatalogErrorKind::UnknownTool,
                tool,
                provider,
                model,
                reasoning,
                known_tools,
                self.policy_provenance.source_label(),
            ));
        }
        let known_providers = self.known_providers(tool);
        if !known_providers
            .iter()
            .any(|known| known == provider || known == "*")
        {
            return Err(Self::error(
                CatalogErrorKind::UnknownProvider,
                tool,
                provider,
                model,
                reasoning,
                known_providers,
                self.policy_provenance.source_label(),
            ));
        }
        Err(Self::error(
            CatalogErrorKind::UnknownModel,
            tool,
            provider,
            model,
            reasoning,
            self.known_models(tool, provider),
            self.policy_provenance.source_label(),
        ))
    }

    pub fn register_configured_spec(
        &mut self,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        provenance: CatalogProvenance,
    ) -> Result<(), ConfiguredSpecError> {
        self.register_configured_sources(tool, provider, model, reasoning, provenance, None)
    }

    /// Register a model selection whose effective reasoning comes from a
    /// distinct setting such as `tools.<name>.thinking_lock`.
    pub fn register_configured_spec_with_reasoning_source(
        &mut self,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        model_provenance: CatalogProvenance,
        reasoning_provenance: CatalogProvenance,
    ) -> Result<(), ConfiguredSpecError> {
        self.register_configured_sources(
            tool,
            provider,
            model,
            reasoning,
            model_provenance,
            Some(reasoning_provenance),
        )
    }

    fn register_configured_sources(
        &mut self,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        model_provenance: CatalogProvenance,
        reasoning_provenance: Option<CatalogProvenance>,
    ) -> Result<(), ConfiguredSpecError> {
        for (field, value) in [("tool", tool), ("provider", provider), ("model", model)] {
            if value.trim().is_empty() {
                return Err(ConfiguredSpecError::EmptyField {
                    field,
                    provenance: model_provenance.source_label(),
                });
            }
            if value != value.trim() {
                return Err(ConfiguredSpecError::SurroundingWhitespace {
                    field,
                    provenance: model_provenance.source_label(),
                });
            }
        }
        let reasoning_source = reasoning_provenance
            .as_ref()
            .unwrap_or(&model_provenance)
            .source_label();
        if reasoning.trim().is_empty() {
            return Err(ConfiguredSpecError::EmptyField {
                field: "reasoning",
                provenance: reasoning_source,
            });
        }
        if reasoning != reasoning.trim() {
            return Err(ConfiguredSpecError::SurroundingWhitespace {
                field: "reasoning",
                provenance: reasoning_source,
            });
        }
        let normalized_reasoning = normalize_configured_reasoning(reasoning).ok_or_else(|| {
            ConfiguredSpecError::InvalidReasoning {
                value: reasoning.to_string(),
                provenance: model_provenance.source_label(),
            }
        })?;
        let provenance = self
            .configured_specs
            .entry(ModelIdentity {
                tool: tool.to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
            })
            .or_default()
            .entry(normalized_reasoning)
            .or_default();
        provenance.model_sources.insert(model_provenance);
        if let Some(reasoning_provenance) = reasoning_provenance {
            provenance.reasoning_sources.insert(reasoning_provenance);
        }
        Ok(())
    }

    /// Resolve the provider for a tool/model override without embedding vendor
    /// identities in Rust. Exact entries (including tombstones) take priority;
    /// otherwise a single catalog scope/provider is used. Ambiguous identities
    /// must be written as `provider/model` by the caller.
    pub fn resolve_provider_for_model(
        &self,
        tool: &str,
        model: &str,
    ) -> Result<String, CatalogResolutionError> {
        let mut exact_providers = BTreeSet::new();
        for identity in self.entries.keys() {
            if identity.tool == tool && identity.model == model {
                exact_providers.insert(identity.provider.clone());
            }
        }
        for identity in self.configured_specs.keys() {
            if identity.tool == tool && identity.model == model {
                exact_providers.insert(identity.provider.clone());
            }
        }
        if exact_providers.len() == 1
            && let Some(provider) = exact_providers.iter().next()
        {
            return Ok(provider.clone());
        }
        if exact_providers.len() > 1 {
            return Err(CatalogResolutionError::AmbiguousProvider {
                tool: tool.to_string(),
                model: model.to_string(),
                providers: exact_providers.into_iter().collect(),
                catalog_source: self.policy_provenance.source_label(),
            });
        }

        let providers: BTreeSet<String> = self
            .open_scopes
            .keys()
            .filter(|scope| scope.tool == tool)
            .map(|scope| scope.provider.clone())
            .collect();

        if providers.is_empty() {
            let known: BTreeSet<String> = self
                .entries
                .keys()
                .filter(|identity| identity.tool == tool)
                .map(|identity| identity.provider.clone())
                .chain(
                    self.open_scopes
                        .keys()
                        .filter(|scope| scope.tool == tool)
                        .map(|scope| scope.provider.clone()),
                )
                .collect();
            if known.len() == 1
                && let Some(provider) = known.into_iter().next()
            {
                return Ok(provider);
            }
            if !self.closed {
                return Ok("*".to_string());
            }
            return Err(CatalogResolutionError::MissingProvider {
                tool: tool.to_string(),
                model: model.to_string(),
                catalog_source: self.policy_provenance.source_label(),
            });
        }
        if providers.len() == 1
            && let Some(provider) = providers.iter().next()
        {
            return Ok(provider.clone());
        }
        Err(CatalogResolutionError::AmbiguousProvider {
            tool: tool.to_string(),
            model: model.to_string(),
            providers: providers.into_iter().collect(),
            catalog_source: self.policy_provenance.source_label(),
        })
    }

    fn matching_scope(&self, tool: &str, provider: &str) -> Option<&CatalogCapability> {
        self.open_scopes
            .get(&ScopeIdentity {
                tool: tool.to_string(),
                provider: provider.to_string(),
            })
            .or_else(|| {
                self.open_scopes.get(&ScopeIdentity {
                    tool: tool.to_string(),
                    provider: "*".to_string(),
                })
            })
    }

    fn validate_capability(
        &self,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        capability: &CatalogCapability,
    ) -> Result<CatalogAdmission, CatalogLegalityError> {
        if let Some(effort) = ReasoningEffort::parse(reasoning) {
            if !capability.reasoning_efforts.contains(&effort) {
                let allowed = capability
                    .reasoning_efforts
                    .iter()
                    .map(|effort| effort.as_str().to_string())
                    .collect();
                return Err(Self::error(
                    CatalogErrorKind::UnsupportedReasoningEffort,
                    tool,
                    provider,
                    model,
                    reasoning,
                    allowed,
                    capability.provenance.source_label(),
                ));
            }
        } else if reasoning.parse::<u32>().is_ok() {
            if !capability.allow_custom_reasoning {
                return Err(Self::error(
                    CatalogErrorKind::UnsupportedCustomReasoning,
                    tool,
                    provider,
                    model,
                    reasoning,
                    Vec::new(),
                    capability.provenance.source_label(),
                ));
            }
        } else {
            return Err(Self::error(
                CatalogErrorKind::UnsupportedReasoningEffort,
                tool,
                provider,
                model,
                reasoning,
                capability
                    .reasoning_efforts
                    .iter()
                    .map(|effort| effort.as_str().to_string())
                    .collect(),
                capability.provenance.source_label(),
            ));
        }
        Ok(CatalogAdmission {
            provenance: capability.provenance.clone(),
            warning: None,
        })
    }

    fn configured_provenance(
        &self,
        identity: &ModelIdentity,
        reasoning: &str,
    ) -> Option<ConfiguredSpecProvenance> {
        let reasoning = normalize_configured_reasoning(reasoning)?;
        self.configured_specs
            .get(identity)
            .and_then(|efforts| efforts.get(&reasoning))
            .cloned()
    }

    fn configured_admission(
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        provenance: ConfiguredSpecProvenance,
        kind: CatalogWarningKind,
    ) -> CatalogAdmission {
        CatalogAdmission {
            provenance: provenance.primary_source(),
            warning: Some(CatalogWarning {
                kind,
                tool: tool.into(),
                provider: provider.into(),
                model: model.into(),
                reasoning: reasoning.into(),
                model_sources: provenance.model_sources.into_iter().collect(),
                reasoning_sources: provenance.reasoning_sources.into_iter().collect(),
            }),
        }
    }

    fn malformed_error(
        field: &'static str,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        source: String,
    ) -> CatalogLegalityError {
        CatalogLegalityError {
            kind: CatalogErrorKind::MalformedIdentity,
            tool: tool.into(),
            provider: provider.into(),
            model: model.into(),
            reasoning: reasoning.into(),
            known: Box::default(),
            source: source.into_boxed_str(),
            malformed_field: Some(field.into()),
        }
    }

    fn error(
        kind: CatalogErrorKind,
        tool: &str,
        provider: &str,
        model: &str,
        reasoning: &str,
        known: Vec<String>,
        source: String,
    ) -> CatalogLegalityError {
        CatalogLegalityError {
            kind,
            tool: tool.into(),
            provider: provider.into(),
            model: model.into(),
            reasoning: reasoning.into(),
            known: known.into_boxed_slice(),
            source: source.into_boxed_str(),
            malformed_field: None,
        }
    }

    pub fn known_tools(&self) -> Vec<String> {
        let mut values = BTreeSet::new();
        values.extend(self.entries.keys().map(|identity| identity.tool.clone()));
        values.extend(self.open_scopes.keys().map(|scope| scope.tool.clone()));
        values.extend(
            self.configured_specs
                .keys()
                .map(|identity| identity.tool.clone()),
        );
        values.into_iter().collect()
    }

    pub fn known_providers(&self, tool: &str) -> Vec<String> {
        let mut values = BTreeSet::new();
        values.extend(
            self.entries
                .keys()
                .filter(|identity| identity.tool == tool)
                .map(|identity| identity.provider.clone()),
        );
        values.extend(
            self.open_scopes
                .keys()
                .filter(|scope| scope.tool == tool)
                .map(|scope| scope.provider.clone()),
        );
        values.extend(
            self.configured_specs
                .keys()
                .filter(|identity| identity.tool == tool)
                .map(|identity| identity.provider.clone()),
        );
        values.into_iter().collect()
    }

    pub fn known_models(&self, tool: &str, provider: &str) -> Vec<String> {
        let mut values: BTreeSet<String> = self
            .entries
            .iter()
            .filter(|(identity, capability)| {
                identity.tool == tool && identity.provider == provider && capability.enabled
            })
            .map(|(identity, _)| identity.model.clone())
            .collect();
        values.extend(
            self.configured_specs
                .keys()
                .filter(|identity| identity.tool == tool && identity.provider == provider)
                .map(|identity| identity.model.clone()),
        );
        values.into_iter().collect()
    }

    pub fn policy_source_label(&self) -> String {
        self.policy_provenance.source_label()
    }
}

fn malformed_identity_field(
    tool: &str,
    provider: &str,
    model: &str,
    reasoning: &str,
) -> Option<&'static str> {
    [
        ("tool", tool),
        ("provider", provider),
        ("model", model),
        ("reasoning", reasoning),
    ]
    .into_iter()
    .find_map(|(field, value)| {
        (value.trim().is_empty() || value != value.trim()).then_some(field)
    })
}

fn normalize_configured_reasoning(value: &str) -> Option<String> {
    if let Some(effort) = ReasoningEffort::parse(value) {
        return Some(effort.as_str().to_string());
    }
    value.parse::<u32>().ok().map(|number| number.to_string())
}
