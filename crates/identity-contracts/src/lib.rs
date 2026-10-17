//! Port of `Identity.Contracts`: authorization names as compiled constants (ADR-0011).
//!
//! Every module passes these to `require_authorization`; none writes the string
//! itself. Each permission is also the name of the policy that requires it,
//! and the value a token carries in its `permissions` claim.

/// Permission claim values, each also a policy of the same name.
pub mod permissions {
    /// Read the Catalog module.
    pub const CATALOG_READ: &str = "catalog.read";
    /// Write the Catalog module. Declared; no endpoint applies it.
    pub const CATALOG_WRITE: &str = "catalog.write";
    /// Read the Orders module.
    pub const ORDERS_READ: &str = "orders.read";
    /// Write the Orders module: finalizing an invoice.
    pub const ORDERS_WRITE: &str = "orders.write";
    /// Manage users. Declared; no endpoint applies it.
    pub const ADMIN_USERS_MANAGE: &str = "admin.users.manage";
    /// Read the Administration module.
    pub const ADMINISTRATION_READ: &str = "administration.read";
    /// Write the Administration module.
    pub const ADMINISTRATION_WRITE: &str = "administration.write";
    /// View reports.
    pub const REPORT_VIEW: &str = "report.view";

    /// Every permission, in upstream's declaration order.
    pub const ALL: [&str; 8] = [
        CATALOG_READ,
        CATALOG_WRITE,
        ORDERS_READ,
        ORDERS_WRITE,
        ADMIN_USERS_MANAGE,
        ADMINISTRATION_READ,
        ADMINISTRATION_WRITE,
        REPORT_VIEW,
    ];
}

/// Policies that are not a single permission.
pub mod policies {
    /// An authenticated caller holding the `Admin` role.
    pub const ADMIN: &str = "role.admin";
    /// An authenticated caller whose `tenant` claim matches the request's tenant.
    pub const TENANT_SCOPED: &str = "tenant.scoped";

    /// Both, in upstream's declaration order.
    pub const ALL: [&str; 2] = [ADMIN, TENANT_SCOPED];
}

/// The role `role.admin` requires.
pub const ADMIN_ROLE: &str = "Admin";

/// The claim permissions travel in.
pub const PERMISSIONS_CLAIM: &str = "permissions";

/// The claim the tenant travels in.
pub const TENANT_CLAIM: &str = "tenant";
