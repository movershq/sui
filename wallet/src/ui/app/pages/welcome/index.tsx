// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useFullscreenGuard, useInitializedGuard } from '_hooks';

const WelcomePage = () => {
    const checkingInitialized = useInitializedGuard(false);
    const guardChecking = useFullscreenGuard();
    return guardChecking || checkingInitialized ? null : <h1>Welcome</h1>;
};

export default WelcomePage;
