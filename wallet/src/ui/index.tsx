// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { createRoot } from 'react-dom/client';
import { Provider } from 'react-redux';

import App from './app';
import store from '~store';

import './styles/global.scss';

const rootDom = document.getElementById('root');
if (!rootDom) {
    throw new Error('Root element not found');
}
const root = createRoot(rootDom);
root.render(
    <Provider store={store}>
        <App />
    </Provider>
);
