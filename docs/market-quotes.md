# Market Quote Read Models

Reusable market quote support is deferred for now.

The current kit already owns the generic pieces that a quote helper would
compose:

- `MarketListing` and `WorldDb::active_listings` for listing stock,
  price, expiry, and metadata.
- owner-keyed inventory for station stock, player-held quantity, and
  generic stockpiles.
- `CapacityPolicy` for game-authored hold, backpack, or warehouse
  capacity.

What remains is highly game-specific: currency ledgers, buy/sell price
policy, demand copy, restock behavior, item metadata, and whether a
listing represents station stock, player listings, or a special service.
Promoting a quote helper before those patterns repeat across downstream
games risks freezing one economy's assumptions into the shared kit.

Revisit this when at least two games duplicate the same small shape:
caller-provided price and balance inputs, listing availability,
player-held quantity, max buy by stock/balance/capacity, max sell by held
quantity, and typed disabled reasons for common blockers. The helper
should remain a read model only and must not encode game-specific
currency, commodities, drift, or ledger rules.
