// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useEffect } from 'react';
import { Navigate, Route, Routes } from 'react-router-dom';

import HomePage from './pages/home';
import WelcomePage from './pages/welcome';
import { useAppDispatch } from '_hooks';
import { loadAccountFromStorage } from '_redux/slices/account';

const App = () => {
    const dispatch = useAppDispatch();
    useEffect(() => {
        dispatch(loadAccountFromStorage());
    });
    return (
        <div className={st.container}>
            <Routes>
                <Route path="/" element={<HomePage />} />
                <Route path="/initialize">
                    <Route index element={<WelcomePage />} />
                </Route>
                <Route path="*" element={<Navigate to="/" replace={true} />} />
            </Routes>
        </div>
    );
};

export default App;
