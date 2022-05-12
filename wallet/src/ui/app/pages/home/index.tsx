// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { Link } from 'react-router-dom';

import { useAppSelector, useInitializedGuard } from '_hooks';
import logo from '_images/sui-icon.png';

import st from './Home.module.scss';

const HomePage = () => {
    const loading = useAppSelector((state) => state.account.loading);
    const mnemonic = useAppSelector((state) => state.account.mnemonic);
    const guardChecking = useInitializedGuard(true);
    return guardChecking ? (
        <>...</>
    ) : (
        <>
            <img className={st.logo} src={logo} alt="logo" />
            <h2>Under Construction</h2>
            <h3>{loading ? 'loading' : `mnemonic: ${mnemonic}`}</h3>
            <Link to="welcome">Welcome screen</Link>
        </>
    );
};

export default HomePage;
