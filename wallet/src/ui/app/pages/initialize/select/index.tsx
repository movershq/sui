// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { Link } from 'react-router-dom';

const SelectPage = () => {
    return (
        <>
            <h1>New to Sui Wallet?</h1>
            <div>
                <div>
                    <h3>Yes, create a new account.</h3>
                    <div>This will create a new wallet and Recovery Phrase</div>
                    <Link to="../create">Create new wallet</Link>
                </div>
                <div>
                    <h3>No, I already have a Recovery Phrase.</h3>
                    <div>
                        Import an existing wallet using a Secret Recovery Phrase
                    </div>
                    <Link to="../import">Import a wallet</Link>
                </div>
            </div>
        </>
    );
};

export default SelectPage;
