-- Payments change updated_at, so keep a separate, stable approval order.
-- Approval assigns the next position while holding the contract row lock.
ALTER TABLE public.contract_parties ADD COLUMN approval_order bigint;

CREATE UNIQUE INDEX contract_parties_approval_order_index
    ON public.contract_parties (contract_id, approval_order);
