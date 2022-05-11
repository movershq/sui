// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useSelector } from 'react-redux';

import type { TypedUseSelectorHook } from 'react-redux';
import type { RootState } from '~store';

const useAppSelector: TypedUseSelectorHook<RootState> = useSelector;

export default useAppSelector;
