use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::account::{Account, AccountError};
use crate::types::{ClientId, DisputeAction, Movement, Transaction, TransactionAction, TxId};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LedgerError {
    #[error("transaction {0} not found")]
    UnknownTransaction(TxId),
    #[error("transaction {0} was submitted by a different client than it belongs to")]
    ClientMismatch(TxId),
    #[error("transaction {0} is already under dispute")]
    AlreadyDisputed(TxId),
    #[error("transaction {0} is not under dispute")]
    NotDisputed(TxId),
    #[error("transaction {0} was already processed")]
    DuplicateTransaction(TxId),
    #[error("withdrawal {0} cannot be disputed")]
    WithdrawalNotDisputable(TxId),
    #[error(transparent)]
    Account(#[from] AccountError),
}

/// The historical record of a deposit or withdrawal, kept so that later
/// dispute/resolve/chargeback transactions can look up the amount, kind,
/// and client they refer to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TxRecord {
    client: ClientId,
    movement: Movement,
    disputed: bool,
}

/// Holds every client's account and enough transaction history to resolve
/// disputes. Built up by repeated calls to [`Ledger::apply`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    // BTreeMap rather than HashMap so that we have a deterministic order
    accounts: BTreeMap<ClientId, Account>,
    history: HashMap<TxId, TxRecord>,
}

impl Ledger {
    pub fn accounts(&self) -> impl Iterator<Item = (ClientId, &Account)> {
        self.accounts
            .iter()
            .map(|(client, account)| (*client, account))
    }

    /// Applies a single transaction to the ledger, mutating account state
    /// in place. Deterministic and free of side effects beyond the
    /// ledger's own state: given the same starting state and transaction,
    /// it always produces the same result.
    pub fn apply(&mut self, transaction: Transaction) -> Result<(), LedgerError> {
        let Transaction { client, tx, action } = transaction;
        match action {
            TransactionAction::Movement(movement) => self.apply_movement(client, tx, movement),
            TransactionAction::DisputeAction(action) => {
                self.apply_dispute_action(client, tx, action)
            }
        }
    }

    /// A deposit or withdrawal: self-contained, doesn't touch history
    /// beyond recording itself.
    fn apply_movement(
        &mut self,
        client: ClientId,
        tx: TxId,
        movement: Movement,
    ) -> Result<(), LedgerError> {
        if self.history.contains_key(&tx) {
            return Err(LedgerError::DuplicateTransaction(tx));
        }
        let mut account = self.account(client);
        match movement {
            Movement::Deposit(amount) => account.deposit(amount)?,
            Movement::Withdrawal(amount) => account.withdraw(amount)?,
        }
        self.accounts.insert(client, account);
        self.history.insert(
            tx,
            TxRecord {
                client,
                movement,
                disputed: false,
            },
        );
        Ok(())
    }

    /// A dispute, resolve, or chargeback: looks up the referenced
    /// movement, applies the corresponding `Account` reaction to a local
    /// copy, and only then commits both the account and the disputed
    /// flag — so a rejection at any step leaves the ledger untouched.
    fn apply_dispute_action(
        &mut self,
        client: ClientId,
        tx: TxId,
        action: DisputeAction,
    ) -> Result<(), LedgerError> {
        let mut account = self.account(client);
        let record = self.find_disputable(client, tx)?;
        match action {
            DisputeAction::Dispute => {
                if record.disputed {
                    return Err(LedgerError::AlreadyDisputed(tx));
                }
                account.hold(record.movement.amount())?;
                record.disputed = true;
            }
            DisputeAction::Resolve => {
                if !record.disputed {
                    return Err(LedgerError::NotDisputed(tx));
                }
                account.release(record.movement.amount())?;
                record.disputed = false;
            }
            DisputeAction::Chargeback => {
                if !record.disputed {
                    return Err(LedgerError::NotDisputed(tx));
                }
                account.chargeback(record.movement.amount())?;
                record.disputed = false;
            }
        }
        self.accounts.insert(client, account);
        Ok(())
    }

    /// The current state of a client's account, or a fresh default one if
    /// the client has no account yet. A plain read: does not touch `self`.
    fn account(&self, client: ClientId) -> Account {
        self.accounts.get(&client).copied().unwrap_or_default()
    }

    /// Looks up a transaction eligible for dispute/resolve/chargeback,
    /// rejecting unknown transactions, transactions belonging to a
    /// different client, and withdrawals (which can't be disputed — see
    /// [`crate::types::Movement`]). Returns a mutable reference so the
    /// caller can flip `disputed` in place once its own fallible steps
    /// succeed, rather than looking the record up a second time to write it.
    fn find_disputable(
        &mut self,
        client: ClientId,
        tx: TxId,
    ) -> Result<&mut TxRecord, LedgerError> {
        let record = self
            .history
            .get_mut(&tx)
            .ok_or(LedgerError::UnknownTransaction(tx))?;
        if record.client != client {
            return Err(LedgerError::ClientMismatch(tx));
        }
        if matches!(record.movement, Movement::Withdrawal(_)) {
            return Err(LedgerError::WithdrawalNotDisputable(tx));
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;
    use rust_decimal::Decimal;

    use super::*;
    use crate::types::Amount;

    /// A withdrawal that fails (here, for insufficient funds) must not
    /// leave any trace in the ledger's transaction history. Before `apply`
    /// was made atomic, the history record was written before the account
    /// operation was attempted, so a failed withdrawal still "existed" for
    /// later dispute/resolve/chargeback transactions to reference.
    #[test]
    fn failed_withdrawal_is_not_recorded() {
        let mut ledger = Ledger::default();
        let client = ClientId::new(1);
        let tx = TxId::new(1);

        let result = ledger.apply(Transaction {
            client,
            tx,
            action: TransactionAction::Movement(Movement::Withdrawal(
                Amount::try_new(Decimal::from_str_exact("10.0").unwrap()).unwrap(),
            )),
        });
        assert_eq!(
            result,
            Err(LedgerError::Account(AccountError::InsufficientFunds))
        );

        let result = ledger.apply(Transaction {
            client,
            tx,
            action: TransactionAction::DisputeAction(DisputeAction::Dispute),
        });
        assert_eq!(result, Err(LedgerError::UnknownTransaction(tx)));
    }

    /// A withdrawal's funds are already gone and held nowhere, so disputing
    /// one must be rejected outright rather than fabricating a hold.
    #[test]
    fn withdrawal_is_not_disputable() {
        let mut ledger = Ledger::default();
        let client = ClientId::new(1);
        let deposit_tx = TxId::new(1);
        let withdrawal_tx = TxId::new(2);
        let amount = Amount::try_new(Decimal::from_str_exact("10.0").unwrap()).unwrap();

        ledger
            .apply(Transaction {
                client,
                tx: deposit_tx,
                action: TransactionAction::Movement(Movement::Deposit(amount)),
            })
            .unwrap();
        ledger
            .apply(Transaction {
                client,
                tx: withdrawal_tx,
                action: TransactionAction::Movement(Movement::Withdrawal(amount)),
            })
            .unwrap();

        let result = ledger.apply(Transaction {
            client,
            tx: withdrawal_tx,
            action: TransactionAction::DisputeAction(DisputeAction::Dispute),
        });
        assert_eq!(
            result,
            Err(LedgerError::WithdrawalNotDisputable(withdrawal_tx))
        );
    }

    /// A small pool of client IDs, so generated transactions frequently
    /// target the same client (needed to exercise dispute/resolve/
    /// chargeback, which only ever succeed against prior history).
    fn arb_client() -> impl Strategy<Value = ClientId> {
        (1..=3u16).prop_map(ClientId::new)
    }

    /// A small pool of tx IDs, so generated disputes/resolves/chargebacks
    /// frequently reference a tx ID that a prior deposit/withdrawal in the
    /// same sequence actually used.
    fn arb_tx() -> impl Strategy<Value = TxId> {
        (1..=8u32).prop_map(TxId::new)
    }

    /// A non-negative amount with up to 4 decimal places, built directly
    /// from an integer mantissa so it's exact (no float rounding).
    fn arb_amount() -> impl Strategy<Value = Amount> {
        (0u32..=1_000_000u32).prop_map(|ten_thousandths| {
            Amount::try_new(Decimal::new(ten_thousandths.into(), 4)).unwrap()
        })
    }

    fn arb_transaction() -> impl Strategy<Value = Transaction> {
        let movement = prop_oneof![
            arb_amount().prop_map(Movement::Deposit),
            arb_amount().prop_map(Movement::Withdrawal),
        ]
        .prop_map(TransactionAction::Movement);
        let dispute_action = prop_oneof![
            Just(DisputeAction::Dispute),
            Just(DisputeAction::Resolve),
            Just(DisputeAction::Chargeback),
        ]
        .prop_map(TransactionAction::DisputeAction);

        (
            arb_client(),
            arb_tx(),
            prop_oneof![movement, dispute_action],
        )
            .prop_map(|(client, tx, action)| Transaction { client, tx, action })
    }

    fn arb_transactions() -> impl Strategy<Value = Vec<Transaction>> {
        prop::collection::vec(arb_transaction(), 0..100)
    }

    /// A client's current account, or a default (zeroed, unlocked) one if
    /// they don't have one yet.
    fn account_of(ledger: &Ledger, client: ClientId) -> Account {
        ledger
            .accounts()
            .find(|(c, _)| *c == client)
            .map(|(_, account)| *account)
            .unwrap_or_default()
    }

    proptest! {
        /// A transaction rejected by `apply` must leave the ledger exactly
        /// as it was, and a client whose account is already locked must
        /// have every subsequent transaction rejected outright.
        #[test]
        fn failed_apply_does_not_mutate_and_locked_accounts_reject_everything(
            transactions in arb_transactions()
        ) {
            let mut ledger = Ledger::default();
            for transaction in transactions {
                let client = transaction.client;
                let was_locked = account_of(&ledger, client).locked();

                let before = ledger.clone();
                let result = ledger.apply(transaction);

                if was_locked {
                    prop_assert!(result.is_err());
                }
                if result.is_err() {
                    prop_assert_eq!(&before, &ledger);
                }
            }
        }

        /// Total funds across all accounts always equals what was actually
        /// deposited, minus what was withdrawn, minus what was charged
        /// back (chargebacks are the only way money leaves the system
        /// after having been counted as deposited).
        #[test]
        fn money_is_conserved(transactions in arb_transactions()) {
            let mut ledger = Ledger::default();
            let mut deposited = Decimal::ZERO;
            let mut withdrawn = Decimal::ZERO;
            let mut charged_back = Decimal::ZERO;
            let mut deposit_amounts: HashMap<TxId, Decimal> = HashMap::new();

            for transaction in transactions {
                let tx = transaction.tx;
                let action = transaction.action;
                let result = ledger.apply(transaction);
                if result.is_err() {
                    continue;
                }
                match action {
                    TransactionAction::Movement(Movement::Deposit(amount)) => {
                        let amount = *amount.as_ref();
                        deposited += amount;
                        deposit_amounts.insert(tx, amount);
                    }
                    TransactionAction::Movement(Movement::Withdrawal(amount)) => {
                        withdrawn += *amount.as_ref();
                    }
                    TransactionAction::DisputeAction(DisputeAction::Chargeback) => {
                        charged_back += deposit_amounts.get(&tx).copied().unwrap_or_default();
                    }
                    TransactionAction::DisputeAction(_) => {}
                }
            }

            let total_in_accounts: Decimal = ledger
                .accounts()
                .map(|(_, account)| account.total().unwrap().into_inner())
                .sum();
            prop_assert_eq!(total_in_accounts, deposited - withdrawn - charged_back);
        }

        /// Held funds only ever move by equal-and-opposite amounts between
        /// dispute (add) and resolve/chargeback (subtract), so they should
        /// never go negative.
        #[test]
        fn held_is_never_negative(transactions in arb_transactions()) {
            let mut ledger = Ledger::default();
            for transaction in transactions {
                let _ = ledger.apply(transaction);
                for (_, account) in ledger.accounts() {
                    prop_assert!(account.held().into_inner() >= Decimal::ZERO);
                }
            }
        }

        /// Replaying the same transaction sequence against two independent
        /// fresh ledgers always produces the same outcomes and final
        /// state: `apply` has no hidden dependency on anything but its
        /// inputs.
        #[test]
        fn ledger_is_deterministic(transactions in arb_transactions()) {
            let mut ledger_a = Ledger::default();
            let mut ledger_b = Ledger::default();
            for transaction in &transactions {
                let result_a = ledger_a.apply(*transaction);
                let result_b = ledger_b.apply(*transaction);
                prop_assert_eq!(result_a.is_ok(), result_b.is_ok());
            }
            prop_assert_eq!(ledger_a, ledger_b);
        }

        /// A successful dispute or resolve only ever moves funds between
        /// `available` and `held`, never changing `total`. A successful
        /// chargeback leaves `available` untouched, reduces `total` by
        /// exactly the disputed deposit's amount, and locks the account.
        #[test]
        fn dispute_family_settles_total_correctly(transactions in arb_transactions()) {
            let mut ledger = Ledger::default();
            let mut deposit_amounts: HashMap<TxId, Decimal> = HashMap::new();

            for transaction in transactions {
                let client = transaction.client;
                let tx = transaction.tx;
                let action = transaction.action;
                let before = account_of(&ledger, client);

                let result = ledger.apply(transaction);

                match action {
                    TransactionAction::Movement(Movement::Deposit(amount)) if result.is_ok() => {
                        deposit_amounts.insert(tx, *amount.as_ref());
                    }
                    TransactionAction::DisputeAction(DisputeAction::Dispute | DisputeAction::Resolve)
                        if result.is_ok() =>
                    {
                        let after = account_of(&ledger, client);
                        prop_assert_eq!(before.total().unwrap(), after.total().unwrap());
                    }
                    TransactionAction::DisputeAction(DisputeAction::Chargeback) if result.is_ok() => {
                        let after = account_of(&ledger, client);
                        let disputed_amount =
                            deposit_amounts.get(&tx).copied().unwrap_or_default();
                        prop_assert_eq!(before.available(), after.available());
                        prop_assert_eq!(
                            after.total().unwrap().into_inner(),
                            before.total().unwrap().into_inner() - disputed_amount
                        );
                        prop_assert!(after.locked());
                    }
                    _ => {}
                }
            }
        }
    }
}
