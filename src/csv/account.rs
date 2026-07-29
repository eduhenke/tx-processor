use std::io::Write;

use serde::Serialize;
use thiserror::Error;

use crate::account::{Account, AccountError};
use crate::types::{Balance, ClientId};

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("failed to write CSV row: {0}")]
    Csv(#[from] csv::Error),
    #[error("failed to compute account total: {0}")]
    Account(#[from] AccountError),
    #[error("failed to flush CSV writer: {0}")]
    Io(#[from] std::io::Error),
}

/// The output CSV's columns, built from an [`Account`] just before being
/// serialized. `total` is computed once here rather than per-field access.
#[derive(Debug, Serialize)]
struct AccountRow {
    client: ClientId,
    available: Balance,
    held: Balance,
    total: Balance,
    locked: bool,
}

impl TryFrom<(ClientId, &Account)> for AccountRow {
    type Error = AccountError;

    fn try_from((client, account): (ClientId, &Account)) -> Result<Self, Self::Error> {
        Ok(AccountRow {
            client,
            available: account.available(),
            held: account.held(),
            total: account.total()?,
            locked: account.locked(),
        })
    }
}

/// Writes one CSV row per account. Streams rows out as they're produced by
/// `accounts` rather than collecting them first, so memory use stays flat
/// regardless of how many accounts there are.
pub fn write_accounts<'a, W: Write>(
    writer: W,
    accounts: impl Iterator<Item = (ClientId, &'a Account)>,
) -> Result<(), WriteError> {
    let mut csv_writer = csv::Writer::from_writer(writer);
    for entry in accounts {
        csv_writer.serialize(AccountRow::try_from(entry)?)?;
    }
    csv_writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::ledger::Ledger;
    use crate::types::{Amount, DisputeAction, Movement, Transaction, TransactionAction, TxId};

    fn amount(value: &str) -> Amount {
        Amount::try_new(Decimal::from_str_exact(value).unwrap()).unwrap()
    }

    #[test]
    fn writes_one_row_per_account() {
        let mut ledger = Ledger::default();
        ledger
            .apply(Transaction {
                client: ClientId::new(1),
                tx: TxId::new(1),
                action: TransactionAction::Movement(Movement::Deposit(amount("1.5"))),
            })
            .unwrap();
        ledger
            .apply(Transaction {
                client: ClientId::new(1),
                tx: TxId::new(2),
                action: TransactionAction::Movement(Movement::Withdrawal(amount("0.5"))),
            })
            .unwrap();
        ledger
            .apply(Transaction {
                client: ClientId::new(2),
                tx: TxId::new(3),
                action: TransactionAction::Movement(Movement::Deposit(amount("2.0"))),
            })
            .unwrap();

        let mut buffer = Vec::new();
        write_accounts(&mut buffer, ledger.accounts()).unwrap();
        let output = String::from_utf8(buffer).unwrap();

        let mut lines: Vec<&str> = output.lines().collect();
        let header = lines.remove(0);
        assert_eq!(header, "client,available,held,total,locked");
        lines.sort_unstable();
        assert_eq!(lines, ["1,1.0,0,1.0,false", "2,2.0,0,2.0,false"]);
    }

    #[test]
    fn locked_account_is_reported_as_locked() {
        let mut ledger = Ledger::default();
        let client = ClientId::new(1);
        let tx = TxId::new(1);
        ledger
            .apply(Transaction {
                client,
                tx,
                action: TransactionAction::Movement(Movement::Deposit(amount("5.0"))),
            })
            .unwrap();
        ledger
            .apply(Transaction {
                client,
                tx,
                action: TransactionAction::DisputeAction(DisputeAction::Dispute),
            })
            .unwrap();
        ledger
            .apply(Transaction {
                client,
                tx,
                action: TransactionAction::DisputeAction(DisputeAction::Chargeback),
            })
            .unwrap();

        let mut buffer = Vec::new();
        write_accounts(&mut buffer, ledger.accounts()).unwrap();
        let output = String::from_utf8(buffer).unwrap();

        assert_eq!(
            output,
            "client,available,held,total,locked\n1,0.0,0.0,0.0,true\n"
        );
    }
}
