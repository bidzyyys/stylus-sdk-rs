// Copyright 2023-2024, Offchain Labs, Inc.
// For licensing, see https://github.com/OffchainLabs/stylus-sdk-rs/blob/main/licenses/COPYRIGHT.md

use crate::{call::CachePolicy, hostio};
use alloc::vec::Vec;
use alloy_primitives::{Address, B256, U256};

#[cfg(feature = "reentrant")]
use crate::storage::StorageCache;

/// Mechanism for performing raw deploys of other contracts.
#[derive(Clone, Default)]
#[must_use]
#[deprecated(
    since = "0.8.0",
    note = "Use the .vm() method available on Stylus contracts instead to access host environment methods"
)]
pub struct RawDeploy {
    salt: Option<B256>,
    #[allow(unused)]
    cache_policy: CachePolicy,
}

#[allow(deprecated)]
impl RawDeploy {
    /// Begin configuring the raw deploy.
    pub fn new() -> Self {
        Default::default()
    }

    /// Configure the deploy to use the salt provided.
    /// This will use [`CREATE2`] under the hood to provide a deterministic address.
    ///
    /// [`CREATE2`]: https://www.evm.codes/#f5
    pub fn salt(mut self, salt: B256) -> Self {
        self.salt = Some(salt);
        self
    }

    /// Configure the deploy to use the salt provided.
    /// This will use [`CREATE2`] under the hood to provide a deterministic address if [`Some`].
    ///
    /// [`CREATE2`]: https://www.evm.codes/#f5
    pub fn salt_option(mut self, salt: Option<B256>) -> Self {
        self.salt = salt;
        self
    }

    /// Write all cached values to persistent storage before the init code.
    #[cfg(feature = "reentrant")]
    pub fn flush_storage_cache(mut self) -> Self {
        self.cache_policy = self.cache_policy.max(CachePolicy::Flush);
        self
    }

    /// Flush and clear the storage cache before the init code.
    #[cfg(feature = "reentrant")]
    pub fn clear_storage_cache(mut self) -> Self {
        self.cache_policy = CachePolicy::Clear;
        self
    }

    /// Performs a raw deploy of another contract with the given `endowment` and init `code`.
    /// Returns the address of the newly deployed contract, or the error data in case of failure.
    ///
    /// # Safety
    ///
    /// Note that the EVM allows init code to make calls to other contracts, which provides a vector for
    /// reentrancy. This means that this method may enable storage aliasing if used in the middle of a storage
    /// reference's lifetime and if reentrancy is allowed.
    ///
    /// For extra flexibility, this method does not clear the global storage cache.
    /// See [`StorageCache::flush`][flush] and [`StorageCache::clear`][clear] for more information.
    ///
    /// [flush]: crate::storage::StorageCache::flush
    /// [clear]: crate::storage::StorageCache::clear
    pub unsafe fn deploy(self, code: &[u8], endowment: U256) -> Result<Address, Vec<u8>> {
        #[cfg(feature = "reentrant")]
        match self.cache_policy {
            CachePolicy::Clear => StorageCache::clear(),
            CachePolicy::Flush => StorageCache::flush(),
            CachePolicy::DoNothing => {}
        }

        let mut contract = Address::default();
        let mut revert_data_len: usize = 0;

        let endowment: B256 = endowment.into();
        if let Some(salt) = self.salt {
            hostio::create2(
                code.as_ptr(),
                code.len(),
                endowment.as_ptr(),
                salt.as_ptr(),
                contract.as_mut_ptr(),
                &mut revert_data_len as *mut _,
            );
        } else {
            hostio::create1(
                code.as_ptr(),
                code.len(),
                endowment.as_ptr(),
                contract.as_mut_ptr(),
                &mut revert_data_len as *mut _,
            );
        }

        if contract.is_zero() {
            #[allow(deprecated)]
            return Err(crate::contract::read_return_data(0, None));
        }
        Ok(contract)
    }
}
