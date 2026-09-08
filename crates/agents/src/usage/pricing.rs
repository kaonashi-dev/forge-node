//! Published token prices, per model (§16.2).
//!
//! A provider-specific fact, so it lives here and nowhere else (principle P2).
//! The table is deliberately small and deliberately incomplete: a model we have
//! no published price for reports *no* price, and the caller counts the turn as
//! unpriced so the view can say the estimate is a floor. Inventing a rate for
//! an unknown model would turn a missing number into a wrong one.
//!
//! Rates are Anthropic first-party list prices in micro-dollars per million
//! tokens, as of 2026-06-24. Cache multipliers follow the documented economics:
//! a read costs 0.1x the input rate, a five-minute write 1.25x, and a one-hour
//! write 2x. Partner platforms (Bedrock, Vertex) bill differently and are not
//! modelled — a transcript does not record which platform served the turn.

/// Micro-dollars in one dollar, mirrored from `domain` for local arithmetic.
const MICROS: u64 = 1_000_000;

/// What one model costs, in micro-dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelPrice {
    pub(super) input: u64,
    pub(super) output: u64,
}

impl ModelPrice {
    const fn new(input_usd: u64, output_usd: u64) -> Self {
        Self {
            input: input_usd * MICROS,
            output: output_usd * MICROS,
        }
    }

    /// Cost of one turn, in micro-dollars.
    ///
    /// `cache_write_1h` is split out because the one-hour TTL is billed at 2x
    /// rather than 1.25x, and Claude's transcripts say which was written.
    pub(super) fn cost_micros(
        self,
        input: u64,
        output: u64,
        cache_write_5m: u64,
        cache_write_1h: u64,
        cache_read: u64,
    ) -> u64 {
        // Scale by 100 before dividing so the 1.25x and 0.1x multipliers stay
        // integral: every term is (tokens * rate * hundredths) / (1e6 * 100).
        let hundredths =
            input * 100 + cache_write_5m * 125 + cache_write_1h * 200 + cache_read * 10;
        (hundredths * self.input) / (MICROS * 100) + (output * self.output) / MICROS
    }
}

/// Price for a model id as the provider writes it, if this build knows one.
///
/// Matching is by substring on the family, not by exact id: providers append
/// and drop suffixes (`claude-opus-5`, `claude-opus-4-8`, a dated snapshot)
/// far more often than they change a family's price tier. Order matters — the
/// first match wins, so more specific families come first.
pub(super) fn price_for(model: &str) -> Option<ModelPrice> {
    /// (family fragment, price). Anthropic list prices, 2026-06-24.
    const TABLE: &[(&str, ModelPrice)] = &[
        ("fable", ModelPrice::new(10, 50)),
        ("mythos", ModelPrice::new(10, 50)),
        ("opus", ModelPrice::new(5, 25)),
        ("sonnet-4-6", ModelPrice::new(3, 15)),
        ("sonnet", ModelPrice::new(2, 10)),
        ("haiku", ModelPrice::new(1, 5)),
    ];

    let model = model.to_ascii_lowercase();
    // Only Anthropic models are priced: the OpenAI and OpenCode rates are not
    // ours to publish from memory, and a transcript on those providers counts
    // as unpriced rather than as free.
    if !model.starts_with("claude") {
        return None;
    }
    TABLE
        .iter()
        .find(|(family, _)| model.contains(family))
        .map(|(_, price)| *price)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_match_by_fragment_not_exact_id() {
        assert_eq!(price_for("claude-opus-5"), Some(ModelPrice::new(5, 25)));
        assert_eq!(price_for("claude-opus-4-8"), Some(ModelPrice::new(5, 25)));
        assert_eq!(price_for("claude-sonnet-5"), Some(ModelPrice::new(2, 10)));
        assert_eq!(price_for("claude-sonnet-4-6"), Some(ModelPrice::new(3, 15)));
        assert_eq!(price_for("claude-haiku-4-5"), Some(ModelPrice::new(1, 5)));
    }

    #[test]
    fn unknown_and_non_anthropic_models_have_no_price() {
        assert_eq!(price_for("gpt-5.6-sol"), None);
        assert_eq!(price_for("claude-something-unreleased"), None);
        assert_eq!(price_for(""), None);
    }

    #[test]
    fn cost_applies_the_documented_cache_multipliers() {
        let opus = ModelPrice::new(5, 25);
        // 1M fresh input at $5.
        assert_eq!(opus.cost_micros(1_000_000, 0, 0, 0, 0), 5 * MICROS);
        // 1M output at $25.
        assert_eq!(opus.cost_micros(0, 1_000_000, 0, 0, 0), 25 * MICROS);
        // 1M cache reads at 0.1x input.
        assert_eq!(opus.cost_micros(0, 0, 0, 0, 1_000_000), MICROS / 2);
        // 1M five-minute cache writes at 1.25x input, one-hour at 2x.
        assert_eq!(
            opus.cost_micros(0, 0, 1_000_000, 0, 0),
            6 * MICROS + MICROS / 4
        );
        assert_eq!(opus.cost_micros(0, 0, 0, 1_000_000, 0), 10 * MICROS);
    }

    #[test]
    fn a_turn_with_no_tokens_costs_nothing() {
        assert_eq!(ModelPrice::new(5, 25).cost_micros(0, 0, 0, 0, 0), 0);
    }
}
