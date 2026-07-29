use nutype::nutype;
use rust_decimal::Decimal;

/// Maximum number of digits past the decimal point supported for amounts.
const AMOUNT_DECIMAL_PLACES: u32 = 4;

#[nutype(derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Display,
    AsRef,
    Deref
))]
pub struct ClientId(u16);

#[nutype(derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Display,
    AsRef,
    Deref
))]
pub struct TxId(u32);

#[nutype(
    validate(predicate = |amount| !amount.is_sign_negative() && amount.scale() <= AMOUNT_DECIMAL_PLACES),
    default = Decimal::ZERO,
    derive(
        Copy,
        Clone,
        Debug,
        Default,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Serialize,
        Deserialize,
        Display,
        AsRef,
        Deref
    )
)]
pub struct Amount(Decimal);

/// A running account balance (available, held, or total funds).
///
/// Unlike [`Amount`], a `Balance` may go negative: disputing a transaction
/// whose funds already moved (e.g. a deposit disputed after the client
/// withdrew against it) holds funds that are no longer actually available,
/// which can drive `available` below zero until the dispute is resolved.
#[nutype(
    validate(predicate = |balance| balance.scale() <= AMOUNT_DECIMAL_PLACES),
    default = Decimal::ZERO,
    derive(
        Copy,
        Clone,
        Debug,
        Default,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Serialize,
        Deserialize,
        Display,
        AsRef,
        Deref
    )
)]
pub struct Balance(Decimal);

impl Balance {
    /// Adds an amount (or another balance) to this balance. Returns `None`
    /// on overflow.
    pub fn checked_add(self, other: impl AsRef<Decimal>) -> Option<Self> {
        let sum = self.into_inner().checked_add(*other.as_ref())?;
        Self::try_new(sum).ok()
    }

    /// Subtracts an amount (or another balance) from this balance. Returns
    /// `None` on underflow. Note this can succeed with a negative result,
    /// unlike [`Amount::checked_sub`].
    pub fn checked_sub(self, other: impl AsRef<Decimal>) -> Option<Self> {
        let diff = self.into_inner().checked_sub(*other.as_ref())?;
        Self::try_new(diff).ok()
    }

    /// Whether this balance is at least `other` (i.e. subtracting `other`
    /// would not make it negative).
    pub fn covers(self, other: impl AsRef<Decimal>) -> bool {
        self.into_inner() >= *other.as_ref()
    }
}

/// A transaction as it appears in the input: which client and tx ID it
/// belongs to, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transaction {
    pub client: ClientId,
    pub tx: TxId,
    pub action: TransactionAction,
}

/// The two families of transaction: money moving in or out of an account,
/// versus a claim about a movement that already happened. They're grouped
/// because `Ledger::apply` treats them very differently — a movement is
/// self-contained, while a dispute action looks up and reacts to history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionAction {
    Movement(Movement),
    DisputeAction(DisputeAction),
}

/// A credit or debit to the client's account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    /// Increases both available and total funds.
    Deposit(Amount),
    /// Decreases both available and total funds. Must fail (be rejected by
    /// the ledger) if available funds are insufficient.
    Withdrawal(Amount),
}

impl Movement {
    pub fn amount(self) -> Amount {
        match self {
            Movement::Deposit(amount) | Movement::Withdrawal(amount) => amount,
        }
    }
}

/// A claim about a previously recorded [`Movement`], identified by the tx
/// ID it refers to (carried on the enclosing [`Transaction`], not here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeAction {
    /// The referenced transaction was erroneous. Moves its amount from
    /// available to held funds; total funds are unaffected.
    Dispute,
    /// Reverses a prior dispute: moves the amount back from held to
    /// available funds; total funds are unaffected.
    Resolve,
    /// The final outcome of a dispute, resolved against the client:
    /// withdraws the held amount, decreasing both held and total funds,
    /// and freezes the client's account.
    Chargeback,
}
