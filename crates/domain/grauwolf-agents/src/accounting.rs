//! Shared counters and limits of a run, including its forks (ADR-0067, ADR-0071, ADR-0072,
//! ADR-0078, ADR-0087).

use std::sync::{Arc, Mutex, MutexGuard};

use grauwolf_llm::LlmError;
use grauwolf_prompt::{
    EstimatingTokenizer, LlModel, Prices, Prompt, Tokenizer, Usage, count_prompt,
};

/// Limits of a run, including forks (ADR-0072); checked before every LLM request.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Budget {
    /// At most this many input and output tokens combined.
    pub max_tokens: Option<u64>,
    /// At most these costs, in the currency of the price table (ADR-0071).
    pub max_cost: Option<f64>,
    /// At most this many tool calls; beyond that the model learns of it as a tool result
    /// (ADR-0078).
    pub max_tool_calls: Option<u64>,
    /// At most this many tokens per request, counted in advance with the tokenizer (ADR-0087).
    pub max_prompt_tokens: Option<u64>,
}

/// Usage, cost and tool calls of a run (shared by forks) and the settings that evaluate them.
pub(crate) struct Accounting {
    /// Sum of the token usage of the run; forks share it (ADR-0067).
    usage: Arc<Mutex<Option<Usage>>>,
    /// Price table for costs (ADR-0071).
    prices: Option<Arc<Prices>>,
    /// Counts tokens before a request (ADR-0087).
    tokenizer: Arc<dyn Tokenizer>,
    /// Limits of the run (ADR-0072).
    budget: Option<Budget>,
    /// Sum of the costs of the run; forks share it (ADR-0071).
    cost: Arc<Mutex<Option<f64>>>,
    /// Executed tool calls of the run; forks share them (ADR-0078).
    tool_calls: Arc<Mutex<u64>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Accounting {
    /// Nothing consumed, no limits, no prices, estimating tokenizer.
    pub(crate) fn new() -> Self {
        Self::resumed(None, None, 0)
    }

    /// Consumption continues from a saved state, so that limits apply to the whole run
    /// (ADR-0077 addendum).
    pub(crate) fn resumed(usage: Option<Usage>, cost: Option<f64>, tool_calls: u64) -> Self {
        Self {
            usage: Arc::new(Mutex::new(usage)),
            prices: None,
            tokenizer: Arc::new(EstimatingTokenizer),
            budget: None,
            cost: Arc::new(Mutex::new(cost)),
            tool_calls: Arc::new(Mutex::new(tool_calls)),
        }
    }

    /// Accounting of a fork: the counters are shared, the settings are copied.
    pub(crate) fn fork(&self) -> Self {
        Self {
            usage: Arc::clone(&self.usage),
            prices: self.prices.clone(),
            tokenizer: Arc::clone(&self.tokenizer),
            budget: self.budget,
            cost: Arc::clone(&self.cost),
            tool_calls: Arc::clone(&self.tool_calls),
        }
    }

    pub(crate) fn set_budget(&mut self, budget: Budget) {
        self.budget = Some(budget);
    }

    pub(crate) fn set_prices(&mut self, prices: Arc<Prices>) {
        self.prices = Some(prices);
    }

    pub(crate) fn set_tokenizer(&mut self, tokenizer: Arc<dyn Tokenizer>) {
        self.tokenizer = tokenizer;
    }

    /// Sum of the token usage; `None` as long as nothing has been reported.
    pub(crate) fn usage(&self) -> Option<Usage> {
        *locked(&self.usage)
    }

    /// Sum of the costs; `None` without a price table or a known model.
    pub(crate) fn cost(&self) -> Option<f64> {
        *locked(&self.cost)
    }

    /// Executed tool calls so far.
    pub(crate) fn tool_calls(&self) -> u64 {
        *locked(&self.tool_calls)
    }

    /// Tokens of `prompt`, as the tokenizer counts them (ADR-0087).
    pub(crate) fn prompt_tokens(&self, prompt: &Prompt) -> u64 {
        count_prompt(self.tokenizer.as_ref(), prompt)
    }

    /// Adds `usage` to the sum of the run; costs at the price of `model`, the model that
    /// answered (ADR-0067, ADR-0071, ADR-0088).
    pub(crate) fn add_usage_for(&self, model: &LlModel, usage: Usage) -> Option<f64> {
        let mut total = locked(&self.usage);
        *total = Some(total.map_or(usage, |total| total + usage));
        let cost = self.prices.as_ref()?.cost(model, &usage)?;
        let mut sum = locked(&self.cost);
        *sum = Some(sum.unwrap_or(0.0) + cost);
        Some(cost)
    }

    /// Error if the run has reached a limit of its budget; `prompt` is the history whose
    /// tokens are checked against `max_prompt_tokens` (ADR-0072).
    pub(crate) fn check_budget(&self, prompt: &Prompt) -> Result<(), LlmError> {
        let Some(budget) = self.budget else {
            return Ok(());
        };
        if let (Some(limit), Some(usage)) = (budget.max_tokens, self.usage()) {
            let used = usage.input_tokens + usage.output_tokens;
            if used >= limit {
                return Err(LlmError::BudgetExceeded {
                    reason: format!("{used} of at most {limit} tokens used"),
                });
            }
        }
        if let Some(limit) = budget.max_prompt_tokens {
            let tokens = self.prompt_tokens(prompt);
            if tokens > limit {
                return Err(LlmError::BudgetExceeded {
                    reason: format!(
                        "the request would have about {tokens} tokens, at most {limit} tokens are allowed"
                    ),
                });
            }
        }
        if let (Some(limit), Some(cost)) = (budget.max_cost, self.cost())
            && cost >= limit
        {
            return Err(LlmError::BudgetExceeded {
                reason: format!("cost {cost:.6} reaches the limit {limit:.6}"),
            });
        }
        Ok(())
    }

    /// Message if the limit for tool calls of the run has been reached (ADR-0078).
    pub(crate) fn tool_limit_reached(&self) -> Option<String> {
        let limit = self.budget.and_then(|budget| budget.max_tool_calls)?;
        (self.tool_calls() >= limit).then(|| tool_limit_message(limit))
    }

    /// Counts a tool call as long as the limit is not reached, under the lock,
    /// so that parallel calls cannot exceed it either (ADR-0078).
    pub(crate) fn claim_tool_call(&self) -> Result<(), String> {
        let limit = self
            .budget
            .and_then(|budget| budget.max_tool_calls)
            .unwrap_or(u64::MAX);
        let mut used = locked(&self.tool_calls);
        if *used >= limit {
            return Err(tool_limit_message(limit));
        }
        *used += 1;
        Ok(())
    }
}

/// Tool result when the limit for tool calls is reached (ADR-0078).
fn tool_limit_message(limit: u64) -> String {
    format!(
        "Error: limit of {limit} tool calls per run reached – answer now without further tools."
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use grauwolf_prompt::ModelPrice;

    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }
    }

    fn model() -> LlModel {
        LlModel::new("test", "m")
    }

    fn prices() -> Arc<Prices> {
        Arc::new(Prices {
            currency: "USD".into(),
            models: BTreeMap::from([(
                "test:m".to_owned(),
                ModelPrice {
                    input: 1.0,
                    output: 2.0,
                    cache_read: None,
                    cache_write: None,
                },
            )]),
        })
    }

    fn prompt(text: &str) -> Prompt {
        Prompt::builder("p").user(text).build()
    }

    fn budget(budget: Budget) -> Accounting {
        let mut accounting = Accounting::new();
        accounting.set_budget(budget);
        accounting
    }

    #[test]
    fn usage_is_none_until_reported_and_then_summed() {
        let accounting = Accounting::new();
        assert_eq!(accounting.usage(), None);
        accounting.add_usage_for(&model(), usage(10, 5));
        accounting.add_usage_for(&model(), usage(1, 2));
        assert_eq!(accounting.usage(), Some(usage(11, 7)));
    }

    #[test]
    fn cost_needs_a_price_table_and_a_known_model() {
        let mut accounting = Accounting::new();
        assert_eq!(
            accounting.add_usage_for(&model(), usage(1_000_000, 0)),
            None
        );
        assert_eq!(accounting.cost(), None);
        accounting.set_prices(prices());
        assert_eq!(
            accounting.add_usage_for(&LlModel::new("test", "other"), usage(1, 1)),
            None
        );
        assert_eq!(accounting.cost(), None);
        let cost = accounting.add_usage_for(&model(), usage(1_000_000, 500_000));
        assert_eq!(cost, Some(2.0));
        accounting.add_usage_for(&model(), usage(1_000_000, 0));
        assert_eq!(accounting.cost(), Some(3.0));
    }

    #[test]
    fn forks_share_counters_but_copy_settings() {
        let mut accounting = budget(Budget {
            max_tool_calls: Some(2),
            ..Budget::default()
        });
        accounting.set_prices(prices());
        let fork = accounting.fork();
        fork.add_usage_for(&model(), usage(1_000_000, 0));
        assert_eq!(accounting.usage(), Some(usage(1_000_000, 0)));
        assert_eq!(accounting.cost(), Some(1.0));
        assert!(accounting.claim_tool_call().is_ok());
        assert!(fork.claim_tool_call().is_ok());
        assert!(accounting.claim_tool_call().is_err());
        assert_eq!(fork.tool_calls(), 2);
    }

    #[test]
    fn resumed_accounting_continues_the_consumption() {
        let accounting = Accounting::resumed(Some(usage(3, 4)), Some(0.5), 7);
        accounting.add_usage_for(&model(), usage(1, 1));
        assert_eq!(accounting.usage(), Some(usage(4, 5)));
        assert_eq!(accounting.cost(), Some(0.5));
        assert_eq!(accounting.tool_calls(), 7);
    }

    #[test]
    fn without_a_budget_nothing_is_limited() {
        let accounting = Accounting::new();
        accounting.add_usage_for(&model(), usage(u64::MAX / 2, 0));
        assert!(accounting.check_budget(&prompt("hi")).is_ok());
        assert_eq!(accounting.tool_limit_reached(), None);
        assert!(accounting.claim_tool_call().is_ok());
    }

    #[test]
    fn token_limit_is_reached_at_the_limit() {
        let accounting = budget(Budget {
            max_tokens: Some(10),
            ..Budget::default()
        });
        accounting.add_usage_for(&model(), usage(6, 3));
        assert!(accounting.check_budget(&prompt("hi")).is_ok());
        accounting.add_usage_for(&model(), usage(1, 0));
        let error = accounting.check_budget(&prompt("hi")).unwrap_err();
        assert!(
            matches!(&error, LlmError::BudgetExceeded { reason } if reason == "10 of at most 10 tokens used"),
            "{error:?}"
        );
    }

    #[test]
    fn cost_limit_is_reached_at_the_limit() {
        let mut accounting = budget(Budget {
            max_cost: Some(1.0),
            ..Budget::default()
        });
        accounting.set_prices(prices());
        accounting.add_usage_for(&model(), usage(500_000, 0));
        assert!(accounting.check_budget(&prompt("hi")).is_ok());
        accounting.add_usage_for(&model(), usage(500_000, 0));
        let error = accounting.check_budget(&prompt("hi")).unwrap_err();
        assert!(
            matches!(&error, LlmError::BudgetExceeded { reason } if reason == "cost 1.000000 reaches the limit 1.000000"),
            "{error:?}"
        );
    }

    struct PerCharacter;

    impl Tokenizer for PerCharacter {
        fn count(&self, text: &str) -> u64 {
            text.chars().count() as u64
        }
    }

    #[test]
    fn prompt_token_limit_uses_the_tokenizer() {
        let mut accounting = budget(Budget {
            max_prompt_tokens: Some(100),
            ..Budget::default()
        });
        accounting.set_tokenizer(Arc::new(PerCharacter));
        let long = prompt(&"x".repeat(200));
        assert_eq!(
            accounting.prompt_tokens(&long),
            count_prompt(&PerCharacter, &long)
        );
        let error = accounting.check_budget(&long).unwrap_err();
        assert!(
            matches!(&error, LlmError::BudgetExceeded { reason } if reason.contains("at most 100 tokens are allowed")),
            "{error:?}"
        );
        assert!(accounting.check_budget(&prompt("short")).is_ok());
    }

    #[test]
    fn tool_calls_are_claimed_up_to_the_limit() {
        let accounting = budget(Budget {
            max_tool_calls: Some(2),
            ..Budget::default()
        });
        assert_eq!(accounting.tool_limit_reached(), None);
        assert!(accounting.claim_tool_call().is_ok());
        assert!(accounting.claim_tool_call().is_ok());
        let message =
            "Error: limit of 2 tool calls per run reached – answer now without further tools.";
        assert_eq!(accounting.claim_tool_call(), Err(message.to_owned()));
        assert_eq!(accounting.tool_limit_reached(), Some(message.to_owned()));
        assert_eq!(accounting.tool_calls(), 2);
    }
}
