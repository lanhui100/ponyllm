//! Integer micro-USD money for the commercial plane.
//!
//! **MONEY_CONTRACT:** money is non-negative integer micro-USD (`u128`,
//! `1 USD = 1_000_000` micro-USD). Floating point, decimals, signed amounts and
//! implicit currency conversion are forbidden at this boundary. Every add,
//! subtract and multiply is checked; overflow, underflow and negative input
//! fail closed.
//!
//! Rounding up per line item is a caller obligation: [`Money::checked_mul`]
//! returns the exact product, and [`Money::ceil_mul_div`] performs the
//! round-up-to-one-micro-unit division required by the contract.

use std::fmt;
use std::str::FromStr;

use crate::error::BillingError;

/// Largest representable amount. `u128::MAX` is exactly `2^128 - 1`, so the
/// exclusive upper bound enforced by the database is `MAX_MICRO_USD + 1`.
pub const MAX_MICRO_USD: u128 = u128::MAX;

/// The exclusive upper bound `2^128`, spelled out for SQL `CHECK` constraints
/// and test assertions.
pub const MONEY_BOUND_EXCLUSIVE_SQL: &str = "340282366920938463463374607431768211456";

/// A non-negative integer amount of micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Money(u128);

impl Money {
    /// Zero micro-USD.
    pub const ZERO: Money = Money(0);

    /// Largest representable amount (`2^128 - 1` micro-USD).
    pub const MAX: Money = Money(MAX_MICRO_USD);

    /// Wrap an already non-negative integer. Cannot fail: every `u128` is
    /// strictly below `2^128`.
    pub const fn from_micro_usd(value: u128) -> Self {
        Money(value)
    }

    /// The raw micro-USD value.
    pub const fn as_micro_usd(self) -> u128 {
        self.0
    }

    /// Parse a decimal integer string. Rejects signs, fractions, scientific
    /// notation, whitespace inside the number, and values `>= 2^128`.
    pub fn parse_micro_usd(raw: &str) -> Result<Self, BillingError> {
        let text = raw.trim();
        if text.is_empty() {
            return Err(BillingError::InvalidAmount {
                reason: "value is empty",
            });
        }
        if text.starts_with('-') {
            return Err(BillingError::NegativeAmount);
        }
        if !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(BillingError::InvalidAmount {
                reason: "value must be a non-negative integer micro-USD string",
            });
        }
        // A value >= 2^128 cannot be represented; `parse` reports overflow.
        text.parse::<u128>()
            .map(Money)
            .map_err(|_| BillingError::AmountOverflow)
    }

    /// Convert a signed integer, rejecting negatives.
    pub fn try_from_i128(value: i128) -> Result<Self, BillingError> {
        if value < 0 {
            return Err(BillingError::NegativeAmount);
        }
        Ok(Money(value as u128))
    }

    /// Checked addition.
    pub fn checked_add(self, other: Money) -> Result<Money, BillingError> {
        self.0
            .checked_add(other.0)
            .map(Money)
            .ok_or(BillingError::AmountOverflow)
    }

    /// Checked subtraction; underflow fails closed.
    pub fn checked_sub(self, other: Money) -> Result<Money, BillingError> {
        self.0
            .checked_sub(other.0)
            .map(Money)
            .ok_or(BillingError::AmountOverflow)
    }

    /// Checked multiplication by a non-negative integer scalar.
    pub fn checked_mul(self, factor: u128) -> Result<Money, BillingError> {
        self.0
            .checked_mul(factor)
            .map(Money)
            .ok_or(BillingError::AmountOverflow)
    }

    /// `ceil(self * numerator / denominator)`, the round-up-to-one-micro-unit
    /// rule. A zero denominator fails closed.
    pub fn ceil_mul_div(self, numerator: u128, denominator: u128) -> Result<Money, BillingError> {
        if denominator == 0 {
            return Err(BillingError::InvalidAmount {
                reason: "divisor must be non-zero",
            });
        }
        let product = self
            .0
            .checked_mul(numerator)
            .ok_or(BillingError::AmountOverflow)?;
        let quotient = product / denominator;
        let remainder = product % denominator;
        let rounded = if remainder == 0 {
            quotient
        } else {
            quotient.checked_add(1).ok_or(BillingError::AmountOverflow)?
        };
        Ok(Money(rounded))
    }

    /// Canonical SQL representation: a decimal integer with no separators.
    pub fn to_sql_string(self) -> String {
        self.0.to_string()
    }
}

impl FromStr for Money {
    type Err = BillingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Money::parse_micro_usd(s)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_sql_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_sql_string() {
        assert_eq!(Money::parse_micro_usd("0").unwrap(), Money::ZERO);
        assert_eq!(Money::parse_micro_usd(" 1000000 ").unwrap().as_micro_usd(), 1_000_000);
        assert_eq!(
            Money::from_micro_usd(42).to_sql_string(),
            "42".to_string()
        );
        assert_eq!(format!("{}", Money::from_micro_usd(7)), "7");
    }

    #[test]
    fn rejects_negative_input() {
        assert!(matches!(
            Money::parse_micro_usd("-1"),
            Err(BillingError::NegativeAmount)
        ));
        assert!(matches!(
            Money::parse_micro_usd("-0"),
            Err(BillingError::NegativeAmount)
        ));
        assert!(matches!(
            Money::try_from_i128(-1),
            Err(BillingError::NegativeAmount)
        ));
        assert_eq!(Money::try_from_i128(0).unwrap(), Money::ZERO);
    }

    #[test]
    fn rejects_fractional_and_non_integer_input() {
        for bad in ["1.5", "1e6", "0x10", "+1", "1 000", "", "  ", "abc", "١٢٣"] {
            assert!(
                matches!(
                    Money::parse_micro_usd(bad),
                    Err(BillingError::InvalidAmount { .. })
                ),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_values_at_or_above_two_to_the_128() {
        // 2^128 - 1 is the largest legal value.
        let below = MAX_MICRO_USD.to_string();
        assert_eq!(below.len(), 39);
        assert_eq!(MONEY_BOUND_EXCLUSIVE_SQL.len(), 39);
        assert_eq!(Money::parse_micro_usd(&below).unwrap(), Money::MAX);

        // 2^128 itself, and everything longer, must be rejected.
        assert!(matches!(
            Money::parse_micro_usd(MONEY_BOUND_EXCLUSIVE_SQL),
            Err(BillingError::AmountOverflow)
        ));
        assert!(matches!(
            Money::parse_micro_usd("999999999999999999999999999999999999999999"),
            Err(BillingError::AmountOverflow)
        ));
    }

    #[test]
    fn checked_arithmetic_fails_closed_on_overflow_and_underflow() {
        assert_eq!(
            Money::from_micro_usd(2)
                .checked_add(Money::from_micro_usd(3))
                .unwrap(),
            Money::from_micro_usd(5)
        );
        assert!(matches!(
            Money::MAX.checked_add(Money::from_micro_usd(1)),
            Err(BillingError::AmountOverflow)
        ));
        assert_eq!(
            Money::from_micro_usd(5)
                .checked_sub(Money::from_micro_usd(2))
                .unwrap(),
            Money::from_micro_usd(3)
        );
        assert!(matches!(
            Money::from_micro_usd(1).checked_sub(Money::from_micro_usd(2)),
            Err(BillingError::AmountOverflow)
        ));
        assert_eq!(
            Money::from_micro_usd(3).checked_mul(4).unwrap(),
            Money::from_micro_usd(12)
        );
        assert!(matches!(
            Money::MAX.checked_mul(2),
            Err(BillingError::AmountOverflow)
        ));
    }

    #[test]
    fn ceil_rounds_up_to_one_micro_unit() {
        // 1 token at 1 micro-USD per 1,000,000 tokens rounds up to 1.
        assert_eq!(
            Money::from_micro_usd(1)
                .ceil_mul_div(1, 1_000_000)
                .unwrap(),
            Money::from_micro_usd(1)
        );
        assert_eq!(
            Money::from_micro_usd(1_000_000)
                .ceil_mul_div(1, 1_000_000)
                .unwrap(),
            Money::from_micro_usd(1)
        );
        assert_eq!(
            Money::from_micro_usd(1_000_001)
                .ceil_mul_div(1, 1_000_000)
                .unwrap(),
            Money::from_micro_usd(2)
        );
        assert!(matches!(
            Money::from_micro_usd(1).ceil_mul_div(1, 0),
            Err(BillingError::InvalidAmount { .. })
        ));
    }
}
