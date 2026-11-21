//! The Tonearm account (not the YouTube Music one, which `settings.rs` has):
//! sign in with an emailed code or a pairing code, see and remove the
//! account's devices, and make a pairing code for another device. All of it
//! is `tonearm_core::account`; every call here blocks on the network, so the
//! app calls them off the main thread.

use crate::{Tonearm, TonearmError};
use tonearm_core::account;

/// The Tonearm account this phone is signed in to.
#[derive(uniffi::Record, Clone)]
pub struct TonearmAccount {
    pub email: String,
}

/// One device of the account.
#[derive(uniffi::Record, Clone)]
pub struct AccountDevice {
    pub id: String,
    pub name: String,
    /// Milliseconds since 1970.
    pub last_seen: i64,
    /// This phone.
    pub current: bool,
}

/// A code for another device to join with, `XXXX-XXXX`.
#[derive(uniffi::Record, Clone)]
pub struct Pairing {
    pub code: String,
    pub expires_in: u32,
}

fn of(account: account::Account) -> TonearmAccount {
    TonearmAccount { email: account.email }
}

#[uniffi::export]
impl Tonearm {
    /// The Tonearm account signed in on this phone, if any. Reads the store only.
    pub fn tonearm_account(&self) -> Option<TonearmAccount> {
        account::current().map(of)
    }

    /// Emails a sign-in code.
    pub fn account_request_code(&self, email: String) -> Result<(), TonearmError> {
        Ok(account::request_code(&email)?)
    }

    /// Signs in with the emailed code.
    pub fn account_sign_in(&self, email: String, code: String) -> Result<TonearmAccount, TonearmError> {
        Ok(of(account::sign_in(&email, &code, "")?))
    }

    /// Signs in with a pairing code another device made.
    pub fn account_pair_claim(&self, code: String) -> Result<TonearmAccount, TonearmError> {
        Ok(of(account::pair_claim(&code, "")?))
    }

    /// Makes a pairing code for another device to join this account with.
    pub fn account_pair_start(&self) -> Result<Pairing, TonearmError> {
        let made = account::pair_start()?;
        Ok(Pairing { code: made.code, expires_in: made.expires_in })
    }

    /// The account's devices, oldest first.
    pub fn account_devices(&self) -> Result<Vec<AccountDevice>, TonearmError> {
        Ok(account::devices()?
            .into_iter()
            .map(|d| AccountDevice { id: d.id, name: d.name, last_seen: d.last_seen, current: d.current })
            .collect())
    }

    /// Signs one device out of the account (this one too, if it is this one).
    pub fn account_remove_device(&self, id: String) -> Result<(), TonearmError> {
        Ok(account::remove_device(&id)?)
    }

    /// Signs this phone out of the Tonearm account.
    pub fn account_sign_out(&self) {
        account::sign_out();
    }
}
