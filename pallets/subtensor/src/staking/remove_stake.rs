use super::*;
use substrate_fixed::types::U96F32;
use subtensor_runtime_common::{AlphaCurrency, Currency, NetUid, TaoCurrency};
use subtensor_swap_interface::{Order, SwapHandler};

impl<T: Config> Pallet<T> {
    /// Internal function that performs unstaking and returns TAO received.
    /// Can be called by chain extensions or other internal code.
    ///
    /// # Arguments
    /// * `coldkey` - The coldkey account (caller/owner)
    /// * `hotkey` - The hotkey to unstake from
    /// * `netuid` - The subnet ID
    /// * `alpha_unstaked` - Amount of ALPHA to unstake
    ///
    /// # Returns
    /// * `Result<TaoCurrency, DispatchError>` - The amount of TAO received from unstaking
    pub fn do_remove_stake_internal(
        coldkey: &T::AccountId,
        hotkey: &T::AccountId,
        netuid: NetUid,
        alpha_unstaked: AlphaCurrency,
    ) -> Result<TaoCurrency, DispatchError> {
        log::debug!(
            "do_remove_stake_internal( coldkey:{coldkey:?} hotkey:{hotkey:?}, netuid: {netuid:?}, alpha_unstaked:{alpha_unstaked:?} )"
        );

        Self::ensure_subtoken_enabled(netuid)?;

        // Cap the alpha_unstaked at available Alpha because user might be paying transaction fees
        // in Alpha and their total is already reduced by now.
        let alpha_available =
            Self::get_stake_for_hotkey_and_coldkey_on_subnet(hotkey, coldkey, netuid);
        let alpha_unstaked = alpha_unstaked.min(alpha_available);

        Self::validate_remove_stake(
            coldkey,
            hotkey,
            netuid,
            alpha_unstaked,
            alpha_unstaked,
            false,
        )?;

        // Swap alpha to TAO and update counters
        let tao_unstaked = Self::unstake_from_subnet(
            hotkey,
            coldkey,
            netuid,
            alpha_unstaked,
            T::SwapInterface::min_price(),
            false,
        )?;

        // Add balance to coldkey
        Self::add_balance_to_coldkey_account(coldkey, tao_unstaked.into());

        // Clear small nomination if required
        Self::clear_small_nomination_if_required(hotkey, coldkey, netuid);

        // Check stake threshold for pending children
        if Self::get_total_stake_for_hotkey(hotkey) < StakeThreshold::<T>::get().into() {
            Self::get_all_subnet_netuids().iter().for_each(|netuid| {
                PendingChildKeys::<T>::remove(netuid, hotkey);
            })
        }

        Ok(tao_unstaked)
    }

    /// ---- The implementation for the extrinsic remove_stake: Removes stake from a hotkey account and adds it onto a coldkey.
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
    /// * 'alpha_unstaked' (Alpha):
    ///     -  The amount of stake to be removed from the staking account.
    ///
    /// # Event:
    /// * StakeRemoved;
    ///     -  On the successfully removing stake from the hotkey account.
    ///
    /// # Raises:
    /// * 'NotRegistered':
    ///     -  Thrown if the account we are attempting to unstake from is non existent.
    ///
    /// * 'NonAssociatedColdKey':
    ///     -  Thrown if the coldkey does not own the hotkey we are unstaking from.
    ///
    /// * 'NotEnoughStakeToWithdraw':
    ///     -  Thrown if there is not enough stake on the hotkey to withdwraw this amount.
    ///
    /// * 'TxRateLimitExceeded':
    ///     -  Thrown if key has hit transaction rate limit
    ///
    pub fn do_remove_stake(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
        netuid: NetUid,
        alpha_unstaked: AlphaCurrency,
    ) -> dispatch::DispatchResult {
        let coldkey = ensure_signed(origin)?;

        // Call internal function, discard return value for extrinsic compatibility
        let _tao_received =
            Self::do_remove_stake_internal(&coldkey, &hotkey, netuid, alpha_unstaked)?;

        Ok(())
    }

    /// Internal function that removes all stake from a hotkey across all subnets and returns total TAO received.
    /// Can be called by chain extensions or other internal code.
    ///
    /// # Arguments
    /// * `coldkey` - The coldkey account (caller/owner)
    /// * `hotkey` - The hotkey to unstake from
    ///
    /// # Returns
    /// * `Result<TaoCurrency, DispatchError>` - The total amount of TAO received from unstaking
    pub fn do_unstake_all_internal(
        coldkey: &T::AccountId,
        hotkey: &T::AccountId,
    ) -> Result<TaoCurrency, DispatchError> {
        log::debug!("do_unstake_all_internal( coldkey:{coldkey:?} hotkey:{hotkey:?} )");

        // Ensure that the hotkey account exists
        ensure!(
            Self::hotkey_account_exists(hotkey),
            Error::<T>::HotKeyAccountNotExists
        );

        // Get all netuids
        let netuids = Self::get_all_subnet_netuids();
        log::debug!("All subnet netuids: {netuids:?}");

        // Track total TAO received
        let mut total_tao_received = TaoCurrency::ZERO;

        // Iterate through all subnets and remove stake
        for netuid in netuids.into_iter() {
            if !SubtokenEnabled::<T>::get(netuid) {
                continue;
            }
            // Get stake amount
            let alpha_unstaked =
                Self::get_stake_for_hotkey_and_coldkey_on_subnet(hotkey, coldkey, netuid);

            if Self::validate_remove_stake(
                coldkey,
                hotkey,
                netuid,
                alpha_unstaked,
                alpha_unstaked,
                false,
            )
            .is_err()
            {
                continue;
            }

            if !alpha_unstaked.is_zero() {
                // Swap alpha to TAO
                let tao_unstaked = Self::unstake_from_subnet(
                    hotkey,
                    coldkey,
                    netuid,
                    alpha_unstaked,
                    T::SwapInterface::min_price(),
                    false,
                )?;

                // Add balance to coldkey
                Self::add_balance_to_coldkey_account(coldkey, tao_unstaked.into());

                // Track total
                total_tao_received = total_tao_received.saturating_add(tao_unstaked);

                // Clear small nomination if required
                Self::clear_small_nomination_if_required(hotkey, coldkey, netuid);
            }
        }

        Ok(total_tao_received)
    }

    /// ---- The implementation for the extrinsic unstake_all: Removes all stake from a hotkey account across all subnets and adds it onto a coldkey.
    ///
    /// # Args:
    /// * 'origin': (<T as frame_system::Config>RuntimeOrigin):
    ///     -  The signature of the caller's coldkey.
    ///
    /// * 'hotkey' (T::AccountId):
    ///     -  The associated hotkey account.
    ///
    /// # Event:
    /// * StakeRemoved;
    ///     -  On the successfully removing stake from the hotkey account.
    ///
    /// # Raises:
    /// * 'NotRegistered':
    ///     -  Thrown if the account we are attempting to unstake from is non existent.
    ///
    /// * 'NonAssociatedColdKey':
    ///     -  Thrown if the coldkey does not own the hotkey we are unstaking from.
    ///
    /// * 'NotEnoughStakeToWithdraw':
    ///     -  Thrown if there is not enough stake on the hotkey to withdraw this amount.
    ///
    /// * 'TxRateLimitExceeded':
    ///     -  Thrown if key has hit transaction rate limit
    ///
    pub fn do_unstake_all(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
    ) -> dispatch::DispatchResult {
        let coldkey = ensure_signed(origin)?;

        // Call internal function, discard return value for extrinsic compatibility
        let _total_tao = Self::do_unstake_all_internal(&coldkey, &hotkey)?;

        Ok(())
    }

    /// Internal function that unstakes all alpha from non-root subnets and stakes into root.
    /// Returns the total TAO equivalent that was converted and staked into root.
    /// Can be called by chain extensions or other internal code.
    ///
    /// # Arguments
    /// * `coldkey` - The coldkey account (caller/owner)
    /// * `hotkey` - The hotkey to unstake from
    ///
    /// # Returns
    /// * `Result<TaoCurrency, DispatchError>` - The total TAO equivalent converted and staked into root
    pub fn do_unstake_all_alpha_internal(
        coldkey: &T::AccountId,
        hotkey: &T::AccountId,
    ) -> Result<TaoCurrency, DispatchError> {
        log::debug!("do_unstake_all_alpha_internal( coldkey:{coldkey:?} hotkey:{hotkey:?} )");

        // Ensure that the hotkey account exists
        ensure!(
            Self::hotkey_account_exists(hotkey),
            Error::<T>::HotKeyAccountNotExists
        );

        // Get all netuids
        let netuids = Self::get_all_subnet_netuids();
        log::debug!("All subnet netuids: {netuids:?}");

        // Track total TAO unstaked
        let mut total_tao_unstaked = TaoCurrency::ZERO;
        for netuid in netuids.into_iter() {
            if !SubtokenEnabled::<T>::get(netuid) {
                continue;
            }
            // If not Root network
            if !netuid.is_root() {
                let alpha_unstaked =
                    Self::get_stake_for_hotkey_and_coldkey_on_subnet(hotkey, coldkey, netuid);

                if Self::validate_remove_stake(
                    coldkey,
                    hotkey,
                    netuid,
                    alpha_unstaked,
                    alpha_unstaked,
                    false,
                )
                .is_err()
                {
                    continue;
                }

                if !alpha_unstaked.is_zero() {
                    // Swap alpha to TAO
                    let tao_unstaked = Self::unstake_from_subnet(
                        hotkey,
                        coldkey,
                        netuid,
                        alpha_unstaked,
                        T::SwapInterface::min_price(),
                        false,
                    )?;

                    // Increment total
                    total_tao_unstaked = total_tao_unstaked.saturating_add(tao_unstaked);

                    // Clear small nomination if required
                    Self::clear_small_nomination_if_required(hotkey, coldkey, netuid);
                }
            }
        }

        // Stake into root
        Self::stake_into_subnet(
            hotkey,
            coldkey,
            NetUid::ROOT,
            total_tao_unstaked,
            T::SwapInterface::max_price(),
            false,
            false,
        )?;

        Ok(total_tao_unstaked)
    }

    /// ---- The implementation for the extrinsic unstake_all: Removes all stake from a hotkey account across all subnets and adds it onto a coldkey.
    ///
    /// # Args:
    /// * 'origin': (<T as frame_system::Config>RuntimeOrigin):
    ///     -  The signature of the caller's coldkey.
    ///
    /// * 'hotkey' (T::AccountId):
    ///     -  The associated hotkey account.
    ///
    /// # Event:
    /// * StakeRemoved;
    ///     -  On the successfully removing stake from the hotkey account.
    ///
    /// # Raises:
    /// * 'NotRegistered':
    ///     -  Thrown if the account we are attempting to unstake from is non existent.
    ///
    /// * 'NonAssociatedColdKey':
    ///     -  Thrown if the coldkey does not own the hotkey we are unstaking from.
    ///
    /// * 'NotEnoughStakeToWithdraw':
    ///     -  Thrown if there is not enough stake on the hotkey to withdraw this amount.
    ///
    /// * 'TxRateLimitExceeded':
    ///     -  Thrown if key has hit transaction rate limit
    ///
    pub fn do_unstake_all_alpha(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
    ) -> dispatch::DispatchResult {
        let coldkey = ensure_signed(origin)?;

        // Call internal function, discard return value for extrinsic compatibility
        let _total_tao = Self::do_unstake_all_alpha_internal(&coldkey, &hotkey)?;

        Ok(())
    }

    /// Internal function that performs unstaking with limit price and returns TAO received.
    /// Can be called by chain extensions or other internal code.
    ///
    /// # Arguments
    /// * `coldkey` - The coldkey account (caller/owner)
    /// * `hotkey` - The hotkey to unstake from
    /// * `netuid` - The subnet ID
    /// * `alpha_unstaked` - Amount of ALPHA to unstake
    /// * `limit_price` - The limit price expressed in units of RAO per one Alpha
    /// * `allow_partial` - Allows partial execution of the amount
    ///
    /// # Returns
    /// * `Result<TaoCurrency, DispatchError>` - The amount of TAO received from unstaking
    pub fn do_remove_stake_limit_internal(
        coldkey: &T::AccountId,
        hotkey: &T::AccountId,
        netuid: NetUid,
        alpha_unstaked: AlphaCurrency,
        limit_price: TaoCurrency,
        allow_partial: bool,
    ) -> Result<TaoCurrency, DispatchError> {
        log::debug!(
            "do_remove_stake_limit_internal( coldkey:{coldkey:?} hotkey:{hotkey:?}, netuid: {netuid:?}, alpha_unstaked:{alpha_unstaked:?} )"
        );

        // Calculate the maximum amount that can be executed with price limit
        let max_amount = Self::get_max_amount_remove(netuid, limit_price)?;
        let mut possible_alpha = alpha_unstaked;
        if possible_alpha > max_amount {
            possible_alpha = max_amount;
        }

        // Validate the user input
        Self::validate_remove_stake(
            coldkey,
            hotkey,
            netuid,
            alpha_unstaked,
            max_amount,
            allow_partial,
        )?;

        // Swap the alpha to tao and update counters for this subnet
        let tao_unstaked = Self::unstake_from_subnet(
            hotkey,
            coldkey,
            netuid,
            possible_alpha,
            limit_price,
            false,
        )?;

        // Add balance to coldkey
        Self::add_balance_to_coldkey_account(coldkey, tao_unstaked.into());

        // Clear small nomination if required
        Self::clear_small_nomination_if_required(hotkey, coldkey, netuid);

        // Check stake threshold for pending children
        if Self::get_total_stake_for_hotkey(hotkey) < StakeThreshold::<T>::get().into() {
            Self::get_all_subnet_netuids().iter().for_each(|netuid| {
                PendingChildKeys::<T>::remove(netuid, hotkey);
            })
        }

        Ok(tao_unstaked)
    }

    /// ---- The implementation for the extrinsic remove_stake_limit: Removes stake from
    /// a hotkey on a subnet with a price limit.
    ///
    /// In case if slippage occurs and the price shall move beyond the limit
    /// price, the staking order may execute only partially or not execute
    /// at all.
    ///
    /// # Args:
    /// * 'origin': (<T as frame_system::Config>Origin):
    ///     - The signature of the caller's coldkey.
    ///
    /// * 'hotkey' (T::AccountId):
    ///     - The associated hotkey account.
    ///
    /// * 'netuid' (u16):
    ///     - Subnetwork UID
    ///
    /// * 'amount_unstaked' (u64):
    ///     - The amount of stake to be added to the hotkey staking account.
    ///
    ///  * 'limit_price' (u64):
    ///     - The limit price expressed in units of RAO per one Alpha.
    ///
    ///  * 'allow_partial' (bool):
    ///     - Allows partial execution of the amount. If set to false, this becomes
    ///       fill or kill type or order.
    ///
    /// # Event:
    /// * StakeRemoved;
    ///     - On the successfully removing stake from the hotkey account.
    ///
    /// # Raises:
    /// * 'NotRegistered':
    ///     - Thrown if the account we are attempting to unstake from is non existent.
    ///
    /// * 'NonAssociatedColdKey':
    ///     - Thrown if the coldkey does not own the hotkey we are unstaking from.
    ///
    /// * 'NotEnoughStakeToWithdraw':
    ///     - Thrown if there is not enough stake on the hotkey to withdwraw this amount.
    ///
    pub fn do_remove_stake_limit(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
        netuid: NetUid,
        alpha_unstaked: AlphaCurrency,
        limit_price: TaoCurrency,
        allow_partial: bool,
    ) -> dispatch::DispatchResult {
        let coldkey = ensure_signed(origin)?;

        // Call internal function, discard return value for extrinsic compatibility
        let _tao_received = Self::do_remove_stake_limit_internal(
            &coldkey,
            &hotkey,
            netuid,
            alpha_unstaked,
            limit_price,
            allow_partial,
        )?;

        Ok(())
    }

    // Returns the maximum amount of RAO that can be executed with price limit
    pub fn get_max_amount_remove(
        netuid: NetUid,
        limit_price: TaoCurrency,
    ) -> Result<AlphaCurrency, DispatchError> {
        // Corner case: root and stao
        // There's no slippage for root or stable subnets, so if limit price is 1e9 rao or
        // lower, then max_amount equals u64::MAX, otherwise it is 0.
        if netuid.is_root() || SubnetMechanism::<T>::get(netuid) == 0 {
            if limit_price <= 1_000_000_000.into() {
                return Ok(AlphaCurrency::MAX);
            } else {
                return Err(Error::<T>::ZeroMaxStakeAmount.into());
            }
        }

        // Use reverting swap to estimate max limit amount
        let order = GetTaoForAlpha::<T>::with_amount(u64::MAX);
        let result = T::SwapInterface::swap(netuid.into(), order, limit_price.into(), false, true)
            .map(|r| r.amount_paid_in.saturating_add(r.fee_paid))?;

        if !result.is_zero() {
            Ok(result)
        } else {
            Err(Error::<T>::ZeroMaxStakeAmount.into())
        }
    }

    pub fn do_remove_stake_full_limit(
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
        netuid: NetUid,
        limit_price: Option<TaoCurrency>,
    ) -> DispatchResult {
        let coldkey = ensure_signed(origin.clone())?;

        let alpha_unstaked =
            Self::get_stake_for_hotkey_and_coldkey_on_subnet(&hotkey, &coldkey, netuid);

        if let Some(limit_price) = limit_price {
            Self::do_remove_stake_limit(origin, hotkey, netuid, alpha_unstaked, limit_price, false)
        } else {
            Self::do_remove_stake(origin, hotkey, netuid, alpha_unstaked)
        }
    }

    pub fn destroy_alpha_in_out_stakes(netuid: NetUid) -> DispatchResult {
        // 1) Ensure the subnet exists.
        ensure!(Self::if_subnet_exist(netuid), Error::<T>::SubnetNotExists);

        // 2) Owner / lock cost.
        let owner_coldkey: T::AccountId = SubnetOwner::<T>::get(netuid);
        let lock_cost: TaoCurrency = Self::get_subnet_locked_balance(netuid);

        // Determine if this subnet is eligible for a lock refund (legacy).
        let reg_at: u64 = NetworkRegisteredAt::<T>::get(netuid);
        let start_block: u64 = NetworkRegistrationStartBlock::<T>::get();
        let should_refund_owner: bool = reg_at < start_block;

        // 3) Compute owner's received emission in TAO at current price (ONLY if we may refund).
        // We:
        //      - get the current alpha issuance,
        //      - apply owner fraction to get owner α,
        //      - price that α using a *simulated* AMM swap.
        let mut owner_emission_tao = TaoCurrency::ZERO;
        if should_refund_owner && !lock_cost.is_zero() {
            let total_emitted_alpha_u128: u128 = Self::get_alpha_issuance(netuid).to_u64() as u128;

            if total_emitted_alpha_u128 > 0 {
                let owner_fraction: U96F32 = Self::get_float_subnet_owner_cut();
                let owner_alpha_u64 = U96F32::from_num(total_emitted_alpha_u128)
                    .saturating_mul(owner_fraction)
                    .floor()
                    .saturating_to_num::<u64>();

                owner_emission_tao = if owner_alpha_u64 > 0 {
                    let cur_price: U96F32 = T::SwapInterface::current_alpha_price(netuid.into());
                    let val_u64 = U96F32::from_num(owner_alpha_u64)
                        .saturating_mul(cur_price)
                        .floor()
                        .saturating_to_num::<u64>();
                    TaoCurrency::from(val_u64)
                } else {
                    TaoCurrency::ZERO
                };
            }
        }

        // 4) Enumerate all α entries on this subnet to build distribution weights and cleanup lists.
        //    - collect keys to remove,
        //    - per (hot,cold) α VALUE (not shares) with fallback to raw share if pool uninitialized,
        //    - track hotkeys to clear pool totals.
        let mut keys_to_remove: Vec<(T::AccountId, T::AccountId)> = Vec::new();
        let mut hotkeys_seen: Vec<T::AccountId> = Vec::new();
        let mut stakers: Vec<(T::AccountId, T::AccountId, u128)> = Vec::new();
        let mut total_alpha_value_u128: u128 = 0;

        for ((hot, cold, this_netuid), share_u64f64) in Alpha::<T>::iter() {
            if this_netuid != netuid {
                continue;
            }

            keys_to_remove.push((hot.clone(), cold.clone()));
            if !hotkeys_seen.contains(&hot) {
                hotkeys_seen.push(hot.clone());
            }

            // Primary: actual α value via share pool.
            let pool = Self::get_alpha_share_pool(hot.clone(), netuid);
            let actual_val_u64 = pool.try_get_value(&cold).unwrap_or(0);

            // Fallback: if pool uninitialized, treat raw Alpha share as value.
            let val_u64 = if actual_val_u64 == 0 {
                share_u64f64.saturating_to_num::<u64>()
            } else {
                actual_val_u64
            };

            if val_u64 > 0 {
                let val_u128 = val_u64 as u128;
                total_alpha_value_u128 = total_alpha_value_u128.saturating_add(val_u128);
                stakers.push((hot, cold, val_u128));
            }
        }

        // 5) Determine the TAO pot and pre-adjust accounting to avoid double counting.
        let pot_tao: TaoCurrency = SubnetTAO::<T>::get(netuid);
        let pot_u64: u64 = pot_tao.into();

        if pot_u64 > 0 {
            SubnetTAO::<T>::remove(netuid);
            TotalStake::<T>::mutate(|total| *total = total.saturating_sub(pot_tao));
        }

        // 6) Pro‑rata distribution of the pot by α value (largest‑remainder),
        //    **credited directly to each staker's COLDKEY free balance**.
        if pot_u64 > 0 && total_alpha_value_u128 > 0 && !stakers.is_empty() {
            struct Portion<A, C> {
                _hot: A,
                cold: C,
                share: u64, // TAO to credit to coldkey balance
                rem: u128,  // remainder for largest‑remainder method
            }

            let pot_u128: u128 = pot_u64 as u128;
            let mut portions: Vec<Portion<_, _>> = Vec::with_capacity(stakers.len());
            let mut distributed: u128 = 0;

            for (hot, cold, alpha_val) in &stakers {
                let prod: u128 = pot_u128.saturating_mul(*alpha_val);
                let share_u128: u128 = prod.checked_div(total_alpha_value_u128).unwrap_or_default();
                let share_u64: u64 = share_u128.min(u128::from(u64::MAX)) as u64;
                distributed = distributed.saturating_add(u128::from(share_u64));

                let rem: u128 = prod.checked_rem(total_alpha_value_u128).unwrap_or_default();
                portions.push(Portion {
                    _hot: hot.clone(),
                    cold: cold.clone(),
                    share: share_u64,
                    rem,
                });
            }

            let leftover: u128 = pot_u128.saturating_sub(distributed);
            if leftover > 0 {
                portions.sort_by(|a, b| b.rem.cmp(&a.rem));
                let give: usize = core::cmp::min(leftover, portions.len() as u128) as usize;
                for p in portions.iter_mut().take(give) {
                    p.share = p.share.saturating_add(1);
                }
            }

            // Credit each share directly to coldkey free balance.
            for p in portions {
                if p.share > 0 {
                    Self::add_balance_to_coldkey_account(&p.cold, p.share);
                }
            }
        }

        // 7) Destroy all α-in/α-out state for this subnet.
        // 7.a) Remove every (hot, cold, netuid) α entry.
        for (hot, cold) in keys_to_remove {
            Alpha::<T>::remove((hot, cold, netuid));
        }
        // 7.b) Clear share‑pool totals for each hotkey on this subnet.
        for hot in hotkeys_seen {
            TotalHotkeyAlpha::<T>::remove(&hot, netuid);
            TotalHotkeyShares::<T>::remove(&hot, netuid);
        }
        // 7.c) Remove α‑in/α‑out counters (fully destroyed).
        SubnetAlphaIn::<T>::remove(netuid);
        SubnetAlphaInProvided::<T>::remove(netuid);
        SubnetAlphaOut::<T>::remove(netuid);

        // Clear the locked balance on the subnet.
        Self::set_subnet_locked_balance(netuid, TaoCurrency::ZERO);

        // 8) Finalize lock handling:
        //    - Legacy subnets (registered before NetworkRegistrationStartBlock) receive:
        //        refund = max(0, lock_cost(τ) − owner_received_emission_in_τ).
        //    - New subnets: no refund.
        let refund: TaoCurrency = if should_refund_owner {
            lock_cost.saturating_sub(owner_emission_tao)
        } else {
            TaoCurrency::ZERO
        };

        if !refund.is_zero() {
            Self::add_balance_to_coldkey_account(&owner_coldkey, refund.to_u64());
        }

        Ok(())
    }
}
