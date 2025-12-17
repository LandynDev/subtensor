use substrate_fixed::types::I96F32;
use subtensor_runtime_common::{AlphaCurrency, NetUid, TaoCurrency};
use subtensor_swap_interface::{Order, SwapHandler};

use super::*;

impl<T: Config> Pallet<T> {
    /// Internal function that performs staking and returns alpha received.
    /// Can be called by chain extensions or other internal code.
    ///
    /// # Arguments
    /// * `coldkey` - The coldkey account (caller/owner)
    /// * `hotkey` - The hotkey to stake to
    /// * `netuid` - The subnet ID
    /// * `stake_to_be_added` - Amount of TAO to stake
    ///
    /// # Returns
    /// * `Result<AlphaCurrency, DispatchError>` - The amount of ALPHA received from staking
    pub fn do_add_stake_internal(
        coldkey: &T::AccountId,
        hotkey: &T::AccountId,
        netuid: NetUid,
        stake_to_be_added: TaoCurrency,
    ) -> Result<AlphaCurrency, DispatchError> {
        log::debug!(
            "do_add_stake_internal( coldkey:{coldkey:?} hotkey:{hotkey:?}, netuid:{netuid:?}, stake_to_be_added:{stake_to_be_added:?} )"
        );

        Self::ensure_subtoken_enabled(netuid)?;

        Self::validate_add_stake(
            coldkey,
            hotkey,
            netuid,
            stake_to_be_added,
            stake_to_be_added,
            false,
        )?;

        let tao_staked: I96F32 =
            Self::remove_balance_from_coldkey_account(coldkey, stake_to_be_added.into())?
                .to_u64()
                .into();

        let alpha_received = Self::stake_into_subnet(
            hotkey,
            coldkey,
            netuid,
            tao_staked.saturating_to_num::<u64>().into(),
            T::SwapInterface::max_price(),
            true,
            false,
        )?;

        Ok(alpha_received)
    }

    /// ---- The implementation for the extrinsic add_stake: Adds stake to a hotkey account.
    ///
    /// # Args:
    /// * 'origin': (<T as frame_system::Config>RuntimeOrigin):
    ///     -  The signature of the caller's coldkey.
    ///
    /// * 'hotkey' (T::AccountId):
    ///     -  The associated hotkey account.
    ///
    /// * 'netuid' (u16):
    ///     - Subnetwork UID
    ///
    /// * 'stake_to_be_added' (u64):
    ///     -  The amount of stake to be added to the hotkey staking account.
    ///
    /// # Event:
    /// * StakeAdded;
    ///     -  On the successfully adding stake to a global account.
    ///
    /// # Raises:
    /// * 'NotEnoughBalanceToStake':
    ///     -  Not enough balance on the coldkey to add onto the global account.
    ///
    /// * 'NonAssociatedColdKey':
    ///     -  The calling coldkey is not associated with this hotkey.
    ///
    /// * 'BalanceWithdrawalError':
    ///     -  Errors stemming from transaction pallet.
    ///
    /// * 'TxRateLimitExceeded':
    ///     -  Thrown if key has hit transaction rate limit
    ///
    pub fn do_add_stake(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
        netuid: NetUid,
        stake_to_be_added: TaoCurrency,
    ) -> dispatch::DispatchResult {
        let coldkey = ensure_signed(origin)?;

        // Call internal function, discard return value for extrinsic compatibility
        let _alpha_received =
            Self::do_add_stake_internal(&coldkey, &hotkey, netuid, stake_to_be_added)?;

        Ok(())
    }

    /// Internal function that performs staking with limit price and returns alpha received.
    /// Can be called by chain extensions or other internal code.
    ///
    /// # Arguments
    /// * `coldkey` - The coldkey account (caller/owner)
    /// * `hotkey` - The hotkey to stake to
    /// * `netuid` - The subnet ID
    /// * `stake_to_be_added` - Amount of TAO to stake
    /// * `limit_price` - The limit price expressed in units of RAO per one Alpha
    /// * `allow_partial` - Allows partial execution of the amount
    ///
    /// # Returns
    /// * `Result<AlphaCurrency, DispatchError>` - The amount of ALPHA received from staking
    pub fn do_add_stake_limit_internal(
        coldkey: &T::AccountId,
        hotkey: &T::AccountId,
        netuid: NetUid,
        stake_to_be_added: TaoCurrency,
        limit_price: TaoCurrency,
        allow_partial: bool,
    ) -> Result<AlphaCurrency, DispatchError> {
        log::debug!(
            "do_add_stake_limit_internal( coldkey:{coldkey:?} hotkey:{hotkey:?}, netuid:{netuid:?}, stake_to_be_added:{stake_to_be_added:?} )"
        );

        // Calculate the maximum amount that can be executed with price limit
        let max_amount: TaoCurrency = Self::get_max_amount_add(netuid, limit_price)?.into();
        let mut possible_stake = stake_to_be_added;
        if possible_stake > max_amount {
            possible_stake = max_amount;
        }

        // Validate user input
        Self::validate_add_stake(
            coldkey,
            hotkey,
            netuid,
            stake_to_be_added,
            max_amount.into(),
            allow_partial,
        )?;

        // If the coldkey is not the owner, make the hotkey a delegate.
        if Self::get_owning_coldkey_for_hotkey(hotkey) != *coldkey {
            Self::maybe_become_delegate(hotkey);
        }

        // Ensure the remove operation from the coldkey is a success.
        let tao_staked = Self::remove_balance_from_coldkey_account(coldkey, possible_stake.into())?;

        // Swap the stake into alpha on the subnet and increase counters.
        let alpha_received = Self::stake_into_subnet(
            hotkey,
            coldkey,
            netuid,
            tao_staked,
            limit_price,
            true,
            false,
        )?;

        Ok(alpha_received)
    }

    /// ---- The implementation for the extrinsic add_stake_limit: Adds stake to a hotkey
    /// account on a subnet with price limit.
    ///
    /// # Args:
    /// * 'origin': (<T as frame_system::Config>RuntimeOrigin):
    ///     -  The signature of the caller's coldkey.
    ///
    /// * 'hotkey' (T::AccountId):
    ///     -  The associated hotkey account.
    ///
    /// * 'netuid' (u16):
    ///     - Subnetwork UID
    ///
    /// * 'stake_to_be_added' (u64):
    ///     -  The amount of stake to be added to the hotkey staking account.
    ///
    ///  * 'limit_price' (u64):
    ///     - The limit price expressed in units of RAO per one Alpha.
    ///
    ///  * 'allow_partial' (bool):
    ///     - Allows partial execution of the amount. If set to false, this becomes
    ///       fill or kill type or order.
    ///
    /// # Event:
    /// * StakeAdded;
    ///     -  On the successfully adding stake to a global account.
    ///
    /// # Raises:
    /// * 'NotEnoughBalanceToStake':
    ///     -  Not enough balance on the coldkey to add onto the global account.
    ///
    /// * 'NonAssociatedColdKey':
    ///     -  The calling coldkey is not associated with this hotkey.
    ///
    /// * 'BalanceWithdrawalError':
    ///     -  Errors stemming from transaction pallet.
    ///
    /// * 'TxRateLimitExceeded':
    ///     -  Thrown if key has hit transaction rate limit
    ///
    pub fn do_add_stake_limit(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
        netuid: NetUid,
        stake_to_be_added: TaoCurrency,
        limit_price: TaoCurrency,
        allow_partial: bool,
    ) -> dispatch::DispatchResult {
        let coldkey = ensure_signed(origin)?;

        // Call internal function, discard return value for extrinsic compatibility
        let _alpha_received = Self::do_add_stake_limit_internal(
            &coldkey,
            &hotkey,
            netuid,
            stake_to_be_added,
            limit_price,
            allow_partial,
        )?;

        Ok(())
    }

    // Returns the maximum amount of RAO that can be executed with price limit
    pub fn get_max_amount_add(
        netuid: NetUid,
        limit_price: TaoCurrency,
    ) -> Result<u64, DispatchError> {
        // Corner case: root and stao
        // There's no slippage for root or stable subnets, so if limit price is 1e9 rao or
        // higher, then max_amount equals u64::MAX, otherwise it is 0.
        if netuid.is_root() || SubnetMechanism::<T>::get(netuid) == 0 {
            if limit_price >= 1_000_000_000.into() {
                return Ok(u64::MAX);
            } else {
                return Err(Error::<T>::ZeroMaxStakeAmount.into());
            }
        }

        // Use reverting swap to estimate max limit amount
        let order = GetAlphaForTao::<T>::with_amount(u64::MAX);
        let result = T::SwapInterface::swap(netuid.into(), order, limit_price, false, true)
            .map(|r| r.amount_paid_in.saturating_add(r.fee_paid))?;

        if !result.is_zero() {
            Ok(result.into())
        } else {
            Err(Error::<T>::ZeroMaxStakeAmount.into())
        }
    }
}
