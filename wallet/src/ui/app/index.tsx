// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useEffect } from 'react';
import { Navigate, Route, Routes } from 'react-router-dom';

import HomePage from './pages/home';
import WelcomePage from './pages/welcome';
import { useAppDispatch } from '~hooks';
import { loadAccountFromStorage } from '~redux/slices/account';

import st from './App.module.scss';

const App = () => {
    const dispatch = useAppDispatch();
    useEffect(() => {
        dispatch(loadAccountFromStorage());
    });
    return (
        <div className={st.container}>
            <Routes>
                <Route path="/" element={<HomePage />} />
                <Route path="welcome" element={<WelcomePage />} />
                <Route path="*" element={<Navigate to="/" replace={true} />} />
            </Routes>
        </div>
    );
};

export default App;
