// SPDX-License-Identifier: GPL-2.0-or-later
//! The license list (`pics.md` §1, PICS-1).

use ossteam_proto::steam::CMsgClientLicenseList;
use ossteam_proto::steam::c_msg_client_license_list::License as Wire;
use ossteam_types::{EResult, PackageId};

/// License flag bits (`pics.md` §1), in our own names.
pub mod flags {
    pub const RENEW: u32 = 0x001;
    pub const RENEWAL_FAILED: u32 = 0x002;
    pub const PENDING: u32 = 0x004;
    pub const EXPIRED: u32 = 0x008;
    pub const CANCELLED_BY_USER: u32 = 0x010;
    pub const CANCELLED_BY_ADMIN: u32 = 0x020;
    pub const LOW_VIOLENCE: u32 = 0x040;
    pub const IMPORTED_FROM_STEAM2: u32 = 0x080;
    pub const FORCE_RUN_RESTRICTION: u32 = 0x100;
    pub const REGION_RESTRICTION_EXPIRED: u32 = 0x200;
    pub const CANCELLED_BY_FRAUD_LOCK: u32 = 0x400;
    pub const NOT_ACTIVATED: u32 = 0x800;

    /// A license with any of these grants nothing.
    pub const VOID: u32 = PENDING
        | EXPIRED
        | CANCELLED_BY_USER
        | CANCELLED_BY_ADMIN
        | CANCELLED_BY_FRAUD_LOCK
        | NOT_ACTIVATED;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct License {
    pub package_id: PackageId,
    pub flags: u32,
    pub license_type: u32,
    /// The account id that owns the license; 0 when the server leaves it
    /// out.
    pub owner_id: u32,
    pub time_created: u32,
    /// The package's PICS access token, 0 when none is needed.
    pub access_token: u64,
}

impl License {
    fn from_wire(l: &Wire) -> Self {
        Self {
            package_id: PackageId(l.package_id()),
            flags: l.flags(),
            license_type: l.license_type(),
            owner_id: l.owner_id(),
            time_created: l.time_created(),
            access_token: l.access_token(),
        }
    }

    /// Whether this license is not in force (`pics.md` §1).
    pub fn is_void(&self) -> bool {
        self.flags & flags::VOID != 0
    }

    /// Whether the license belongs to another account (Family Sharing).
    pub fn is_borrowed(&self, own_account_id: u32) -> bool {
        self.owner_id != 0 && self.owner_id != own_account_id
    }

    /// Whether the license puts its package in this account's library.
    pub fn grants(&self, own_account_id: u32) -> bool {
        !self.is_void() && !self.is_borrowed(own_account_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LicenseListError {
    #[error("the license list does not decode: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("the server could not send the license list: {0}")]
    Failed(EResult),
}

/// Reads a `ClientLicenseList` body.
pub fn parse(body: &[u8]) -> Result<Vec<License>, LicenseListError> {
    let msg: CMsgClientLicenseList = prost::Message::decode(body)?;
    let eresult = EResult(msg.eresult());
    if !eresult.is_ok() {
        return Err(LicenseListError::Failed(eresult));
    }
    Ok(msg.licenses.iter().map(License::from_wire).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn wire(package_id: u32, flags: u32, owner_id: u32) -> Wire {
        Wire {
            package_id: Some(package_id),
            flags: Some(flags),
            owner_id: Some(owner_id),
            access_token: Some(7),
            ..Default::default()
        }
    }

    #[test]
    fn parses_and_classifies() {
        let body = CMsgClientLicenseList {
            eresult: Some(1),
            licenses: vec![
                wire(10, flags::RENEW, 5),
                wire(11, flags::EXPIRED, 5),
                wire(12, 0, 6),
                wire(13, flags::LOW_VIOLENCE, 0),
            ],
        }
        .encode_to_vec();
        let l = parse(&body).unwrap();
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].access_token, 7);
        let granted: Vec<u32> = l.iter().filter(|l| l.grants(5)).map(|l| l.package_id.0).collect();
        assert_eq!(granted, [10, 13]);
        assert!(l[2].is_borrowed(5));
    }

    #[test]
    fn a_failed_list_is_an_error() {
        let body = CMsgClientLicenseList {
            eresult: Some(2),
            licenses: vec![],
        }
        .encode_to_vec();
        assert!(matches!(parse(&body), Err(LicenseListError::Failed(_))));
        // The default eresult is Fail (2), so an empty body is a failure too.
        assert!(parse(&[]).is_err());
    }
}
