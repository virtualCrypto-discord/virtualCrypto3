-- Fine-grained personal permissions. Existing JWT and guild scopes retain their meanings.
-- Old personal scopes are not aliases; clients must request explicit permissions.
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.profile.read';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.balances.read';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.claims.read';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.contracts.read';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.contracts.payments.read';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.payments.create';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.claims.create';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.claims.approve';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.claims.deny';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.claims.cancel';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.delegate.claims.metadata.write';
