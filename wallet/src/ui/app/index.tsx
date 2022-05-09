// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useEffect } from 'react';

import { useAppDispatch, useAppSelector } from './hooks';
import { loadAccountFromStorage } from './redux/slices/account';
import logo from '~images/sui-icon.png';

import st from './App.module.scss';

const App = () => {
    const dispatch = useAppDispatch();
    useEffect(() => {
        dispatch(loadAccountFromStorage());
    });
    const loading = useAppSelector((state) => state.account.loading);
    const mnemonic = useAppSelector((state) => state.account.mnemonic);
    return (
        <div className={st.container}>
            <img className={st.logo} src={logo} alt="logo" />
            <h2>Under Construction</h2>
            <h3>{loading ? 'loading' : `mnemonic: ${mnemonic}`}</h3>
        </div>
    );
};

export default App;
