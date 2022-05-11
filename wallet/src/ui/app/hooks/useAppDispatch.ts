// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useDispatch } from 'react-redux';

import type { AppDispatch } from '~store';

export default function useAppDispatch() {
    return useDispatch<AppDispatch>();
}
