import { createAsyncThunk, createSlice } from '@reduxjs/toolkit';
import Browser from 'webextension-polyfill';

export const loadAccountFromStorage = createAsyncThunk(
    'account/loadAccount',
    async (): Promise<string> => {
        const { mnemonic } = await Browser.storage.local.get('account');
        return mnemonic;
    }
);

type AccountState = {
    loading: boolean;
    mnemonic: string | null;
};

const initialState: AccountState = {
    loading: true,
    mnemonic: null,
};

const accountSlice = createSlice({
    name: 'account',
    initialState,
    reducers: {},
    extraReducers: (builder) =>
        builder.addCase(loadAccountFromStorage.fulfilled, (state, action) => {
            state.loading = false;
            state.mnemonic = action.payload;
        }),
});

export default accountSlice.reducer;
