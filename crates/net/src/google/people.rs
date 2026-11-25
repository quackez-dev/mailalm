//! The People API (v1), read only: the user's contacts, to complete addresses (PLAN.md, K1).

use mailalm_model::pim::Contact;

use super::{Api, GoogleClient, convert};
use crate::Result;

/// Most contacts taken.
const MAX_CONTACTS: usize = 50_000;
/// What is asked of each contact.
const PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations";

impl GoogleClient {
    /// The user's contacts (`people/me/connections`), page by page, at most [`MAX_CONTACTS`];
    /// one without a name or an address is left out.
    pub fn contacts(&mut self) -> Result<Vec<Contact>> {
        let first = format!("{}/people/me/connections?personFields={PERSON_FIELDS}&pageSize=1000", self.base(Api::People));
        let items = self.pages(Api::People, &first, "connections", MAX_CONTACTS)?;
        Ok(items.iter().filter_map(convert::contact_of).collect())
    }
}
