// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useCallback } from 'react';

import { useAppDispatch, useAppSelector } from '_src/ui/app/hooks';
import { createMnemonic } from '_src/ui/app/redux/slices/account';

const CreatePage = () => {
    const dispatch = useAppDispatch();
    const onHandleCreate = useCallback(async () => {
        await dispatch(createMnemonic());
    }, [dispatch]);
    const createdMnemonic = useAppSelector(
        (state) => state.account.createdMnemonic
    );
    return (
        <>
            <h1>Create new wallet</h1>
            <div>
                Creating a wallet generates a Recovery Passphrase. Using it you
                can restore the wallet.
            </div>
            <div>{createdMnemonic}</div>
            <label>
                <input type="checkbox" />I have read and agree to the{' '}
                <a href="https://sui.io/terms" target="_blank" rel="noreferrer">
                    Terms of Service
                </a>
            </label>
            <div>
                <button type="button" onClick={onHandleCreate}>
                    Create
                </button>
            </div>
        </>
    );
};

export default CreatePage;
