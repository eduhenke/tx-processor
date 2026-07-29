use thiserror::Error;

use crate::types::{Amount, Balance};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccountError {
    #[error("account is locked")]
    Locked,
    #[error("insufficient available funds")]
    InsufficientFunds,
    #[error("balance overflow")]
    Overflow,
}

/// A single client's asset account: available funds, funds held for
/// dispute, and whether the account has been frozen by a chargeback.
///
/// `total` is not stored; it is always derived as `available + held`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Account {
    available: Balance,
    held: Balance,
    locked: bool,
}

impl Account {
    pub fn available(&self) -> Balance {
        self.available
    }

    pub fn held(&self) -> Balance {
        self.held
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn total(&self) -> Result<Balance, AccountError> {
        self.available
            .checked_add(self.held)
            .ok_or(AccountError::Overflow)
    }

    /// Credits `amount` to available funds.
    pub fn deposit(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        self.available = self
            .available
            .checked_add(amount)
            .ok_or(AccountError::Overflow)?;
        Ok(())
    }

    /// Debits `amount` from available funds. Fails if the account doesn't
    /// have enough available funds to cover it.
    pub fn withdraw(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        if !self.available.covers(amount) {
            return Err(AccountError::InsufficientFunds);
        }
        self.available = self
            .available
            .checked_sub(amount)
            .ok_or(AccountError::Overflow)?;
        Ok(())
    }

    /// Moves `amount` from available to held, as a dispute is opened.
    /// Available may go negative if the disputed funds already moved
    /// elsewhere (e.g. a deposit disputed after being withdrawn against).
    pub fn hold(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        self.available = self
            .available
            .checked_sub(amount)
            .ok_or(AccountError::Overflow)?;
        self.held = self
            .held
            .checked_add(amount)
            .ok_or(AccountError::Overflow)?;
        Ok(())
    }

    /// Moves `amount` from held back to available, as a dispute is
    /// resolved in the client's favor.
    pub fn release(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        self.held = self
            .held
            .checked_sub(amount)
            .ok_or(AccountError::Overflow)?;
        self.available = self
            .available
            .checked_add(amount)
            .ok_or(AccountError::Overflow)?;
        Ok(())
    }

    /// Withdraws `amount` from held funds and freezes the account, as a
    /// dispute is resolved against the client.
    pub fn chargeback(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        self.held = self
            .held
            .checked_sub(amount)
            .ok_or(AccountError::Overflow)?;
        self.locked = true;
        Ok(())
    }

    fn ensure_unlocked(&self) -> Result<(), AccountError> {
        if self.locked {
            return Err(AccountError::Locked);
        }
        Ok(())
    }
}
