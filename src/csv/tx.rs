use std::io::Read;
use std::path::Path;

use rust_decimal::Decimal;
use serde::Deserialize;
use thiserror::Error;

use crate::types::{
    Amount, AmountError, ClientId, DisputeAction, Movement, Transaction, TransactionAction, TxId,
};

#[derive(Debug, Error)]
pub enum ReadError {
    /// Covers I/O failures (e.g. the file can't be opened), malformed CSV
    /// structure, and an unrecognized `type` value, since all of these
    /// surface as a `csv::Error` from either opening the reader or
    /// deserializing a [`Row`].
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("transaction {0}: {1} requires an amount")]
    MissingAmount(TxId, &'static str),
    #[error("transaction {0}: invalid amount: {1}")]
    InvalidAmount(TxId, AmountError),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RowKind {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// The CSV file's columns, deserialized as-is before being validated and
/// converted into a [`Transaction`]. `amount` is only present for deposits
/// and withdrawals; the `csv` crate maps an empty field to `None` for an
/// `Option<T>` column.
#[derive(Debug, Deserialize)]
struct Row {
    #[serde(rename = "type")]
    kind: RowKind,
    client: ClientId,
    tx: TxId,
    amount: Option<Decimal>,
}

impl TryFrom<Row> for Transaction {
    type Error = ReadError;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let Row {
            kind,
            client,
            tx,
            amount,
        } = row;

        let amount_for = |kind: &'static str| -> Result<Amount, ReadError> {
            let decimal = amount.ok_or(ReadError::MissingAmount(tx, kind))?;
            Amount::try_new(decimal).map_err(|source| ReadError::InvalidAmount(tx, source))
        };

        let action = match kind {
            RowKind::Deposit => {
                TransactionAction::Movement(Movement::Deposit(amount_for("deposit")?))
            }
            RowKind::Withdrawal => {
                TransactionAction::Movement(Movement::Withdrawal(amount_for("withdrawal")?))
            }
            RowKind::Dispute => TransactionAction::DisputeAction(DisputeAction::Dispute),
            RowKind::Resolve => TransactionAction::DisputeAction(DisputeAction::Resolve),
            RowKind::Chargeback => TransactionAction::DisputeAction(DisputeAction::Chargeback),
        };

        Ok(Transaction { client, tx, action })
    }
}

/// Streams [`Transaction`]s out of a CSV source one row at a time.
///
/// Backed by the `csv` crate's own incremental reader, which parses and
/// yields a single record per `next()` call rather than buffering the
/// whole file, so memory use stays flat regardless of input size.
pub fn read_transactions(
    path: impl AsRef<Path>,
) -> Result<impl Iterator<Item = Result<Transaction, ReadError>>, ReadError> {
    let csv_reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_path(path)?;
    Ok(transactions_from(csv_reader))
}

fn transactions_from<R: Read>(
    csv_reader: csv::Reader<R>,
) -> impl Iterator<Item = Result<Transaction, ReadError>> {
    csv_reader
        .into_deserialize::<Row>()
        .map(|row| row.map_err(ReadError::from).and_then(Transaction::try_from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(csv: &str) -> Vec<Result<Transaction, ReadError>> {
        let csv_reader = csv::ReaderBuilder::new()
            .trim(csv::Trim::All)
            .from_reader(csv.as_bytes());
        transactions_from(csv_reader).collect()
    }

    #[test]
    fn parses_every_transaction_type() {
        let csv = "\
type, client, tx, amount
deposit, 1, 1, 1.0
withdrawal, 1, 2, 0.5
dispute, 1, 1,
resolve, 1, 1,
chargeback, 1, 1,
";
        let transactions: Vec<Transaction> =
            read_all(csv).into_iter().map(Result::unwrap).collect();

        let client = ClientId::new(1);
        assert_eq!(
            transactions,
            vec![
                Transaction {
                    client,
                    tx: TxId::new(1),
                    action: TransactionAction::Movement(Movement::Deposit(
                        Amount::try_new(Decimal::new(10, 1)).unwrap()
                    )),
                },
                Transaction {
                    client,
                    tx: TxId::new(2),
                    action: TransactionAction::Movement(Movement::Withdrawal(
                        Amount::try_new(Decimal::new(5, 1)).unwrap()
                    )),
                },
                Transaction {
                    client,
                    tx: TxId::new(1),
                    action: TransactionAction::DisputeAction(DisputeAction::Dispute),
                },
                Transaction {
                    client,
                    tx: TxId::new(1),
                    action: TransactionAction::DisputeAction(DisputeAction::Resolve),
                },
                Transaction {
                    client,
                    tx: TxId::new(1),
                    action: TransactionAction::DisputeAction(DisputeAction::Chargeback),
                },
            ]
        );
    }

    #[test]
    fn rejects_deposit_without_amount() {
        let csv = "type, client, tx, amount\ndeposit, 1, 1,\n";
        let results = read_all(csv);
        assert!(matches!(
            results.as_slice(),
            [Err(ReadError::MissingAmount(tx, "deposit"))] if *tx == TxId::new(1)
        ));
    }

    #[test]
    fn rejects_unknown_transaction_type() {
        let csv = "type, client, tx, amount\nteleport, 1, 1, 1.0\n";
        let results = read_all(csv);
        assert!(matches!(results.as_slice(), [Err(ReadError::Csv(_))]));
    }

    #[test]
    fn does_not_buffer_the_whole_input_upfront() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingReader<'a> {
            remaining: &'a [u8],
            bytes_read: &'a AtomicUsize,
        }

        impl Read for CountingReader<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.remaining.read(buf)?;
                self.bytes_read.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
        }

        // Comfortably larger than any internal read buffer, so a single
        // `next()` call can only have consumed a small prefix of it if
        // rows are genuinely streamed rather than loaded upfront.
        let mut csv = String::from("type, client, tx, amount\n");
        for i in 1..=100_000u32 {
            csv.push_str(&format!("deposit, 1, {i}, 1.0\n"));
        }

        let bytes_read = AtomicUsize::new(0);
        let csv_reader =
            csv::ReaderBuilder::new()
                .trim(csv::Trim::All)
                .from_reader(CountingReader {
                    remaining: csv.as_bytes(),
                    bytes_read: &bytes_read,
                });
        let mut reader = transactions_from(csv_reader);

        assert!(reader.next().unwrap().is_ok());

        let consumed = bytes_read.load(Ordering::SeqCst);
        assert!(
            consumed < csv.len(),
            "first next() call read {consumed} of {} total bytes; the whole file was buffered upfront",
            csv.len()
        );
    }
}
