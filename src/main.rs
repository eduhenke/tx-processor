mod account;
mod csv;
mod ledger;
mod types;

use std::env;
use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

use ledger::Ledger;

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: tx-processor <transactions.csv>");
        return ExitCode::FAILURE;
    };

    let transactions = match csv::tx::read_transactions(&path) {
        Ok(transactions) => transactions,
        Err(error) => {
            eprintln!("failed to open {path}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut ledger = Ledger::default();
    for result in transactions {
        match result {
            Ok(transaction) => {
                let (client, tx) = (transaction.client, transaction.tx);
                if let Err(error) = ledger.apply(transaction) {
                    eprintln!("skipping transaction {tx} for client {client}: {error}");
                }
            }
            Err(error) => eprintln!("skipping malformed row: {error}"),
        }
    }

    let mut stdout = BufWriter::new(io::stdout());
    if let Err(error) = csv::account::write_accounts(&mut stdout, ledger.accounts()) {
        eprintln!("failed to write output: {error}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = stdout.flush() {
        eprintln!("failed to flush output: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
