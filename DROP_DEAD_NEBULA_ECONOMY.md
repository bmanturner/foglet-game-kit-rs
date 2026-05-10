# Drop Dead Nebula — Economy Deep Dive

Status: Draft v0.1  
Primary milestone relevance: static M0, simple drift M2, broad economy M10  
Purpose: Define how Commodities, Markets, prices, stock, NPC flow, scarcity, and player manipulation create strategic trade.

---

## 1. Design Intent

The economy is the strategic nervous system.

It should:

- create reasons to travel;
- support multiple careers;
- react to players and NPCs;
- generate jobs;
- make information valuable;
- avoid solved infinite loops;
- remain readable.

It should not:

- require a spreadsheet;
- be static after route discovery;
- need realtime simulation;
- hide all causes from players.

---

## 2. Milestone Placement

| Milestone | Economy Role |
| --- | --- |
| M0 | Static Med Gel trade route. |
| M1 | Better Market UX. |
| M2 | Restock/reprice and background trade. |
| M3 | Route risk affects trade. |
| M4 | Salvage introduces non-market acquisition. |
| M5 | Jobs/Bounties arise from demand. |
| M6 | Modules, repairs, fuel, contraband, Heat. |
| M7 | Faction pricing/access/shared demand. |
| M9 | Outpost stockpiles. |
| M10 | Production/consumption, player listings, more ports. |
| M11 | Campaign shocks and regional economy. |

---

## 3. Economic Pillars

### Legibility

Players should know broad reasons for prices: shortage, surplus, route danger, faction control, crisis, distance, black-market risk.

### Information Advantage

Profit comes from knowing demand, stale intel, route safety, restock timing, NPC competition, and Faction needs.

### Risk-Adjusted Profit

High margins should imply danger, Heat, scarcity, turn cost, capacity lockup, or competition.

### Low-Pop Motion

World Ticks and NPC flows keep Markets moving even with one player.

---

## 4. Commodity Families

- **Staples:** Food Paste, Water, Med Gel, Oxygen Candles.
- **Industrial:** Ore, Reactor Coolant, Machine Parts, Station Glass, Relay Coils.
- **Technology:** Nav Cores, Sensor Wafers, Drone Brains, Shield Emitters.
- **Illicit:** Black AI Fragments, Forged Transponders, Weapon Crates, Unlicensed Relics.
- **Strategic:** Terraforming Kits, Defense Grids, Jump-Gate Anchors, Colony Seedbanks.
- **Relic/Exotic:** Dead-Star Amber, Choir Metal, Memory Fossils, Bottled Void.

Each family should support different jobs, risks, and destinations.

---

## 5. Port Archetypes

### Refinery Station

Produces coolant/industrial goods; demands Ore and supplies. Example: Ash Coil.

### Relay / Humanitarian Station

Produces intel/jobs; demands Med Gel, Coolant, Relay Coils. Example: Mercy Relay.

### Mining Rock

Produces Ore; demands Food, Med Gel, Parts, protection. Example: Cinder Pocket.

### Free Port / Black Market

Produces illicit listings and rumors; demands contraband/relics/favors. Example: Saint Vex Drift later.

### Shipyard

Sells repairs/modules/hulls; demands credits, alloys, tech goods.

### Faction Enclave

Offers restricted jobs/services; demands shared-goal goods.

---

## 6. Price Model, Design-Level

### M0: Authored Prices

Simple: Ash Coil sells Med Gel, Mercy Relay buys it profitably.

### M2: Stock-Sensitive Drift

If stock is below equilibrium, buy price rises. If above equilibrium, sell price falls. Station archetype sets normal tendencies.

Player copy:

```text
Mercy Relay is short on Med Gel. Prices are up.
```

### M3+: Risk Modifier

Dangerous routes raise destination prices. Customs pressure affects illegal goods. Pirate pressure raises weapons/repair demand.

### M7+: Faction Modifier

Standing and control affect access, prices, and job rewards.

### Campaign/Crisis Modifier

Plague, reactor leak, refugee wave, relic rush, blockade, or faction war temporarily alter demand.

---

## 7. Stock, Equilibrium, Consumption, Production

Each Market owns stock. Each station has equilibrium targets. World Ticks process:

- consumption by station needs;
- production by archetype/outposts;
- NPC flow transfers;
- restock toward equilibrium;
- price labels/digest lines.

Stock matters because players can buy out goods, satisfy demand, create shortages, or flood Markets.

---

## 8. NPC Economic Activity

Background flow examples:

- Cinder Pocket produces Ore.
- Merchants move some Ore to Ash Coil.
- Ash Coil produces Coolant.
- Some Coolant moves to Mercy Relay.

Named NPC events:

```text
Brass Jory dumped coolant at Ash Coil, depressing prices.
Kara Vex bought out forged transponders at Saint Vex Drift.
```

NPCs should affect Markets occasionally, not constantly.

---

## 9. Anti-Farming

Use world reactions, not invisible punishment:

- stock depletion;
- demand satisfaction;
- margin compression;
- NPC competition;
- route hazard increase;
- customs attention;
- contract expiry;
- cargo and turn opportunity cost.

Good copy:

```text
Mercy Relay's med bays are stocked for now. Med Gel prices cooled.
```

---

## 10. Market Manipulation

Players should affect the economy:

- buy out stock;
- flood Markets;
- stockpile before crisis;
- supply Faction goals;
- use Outposts as buffers;
- disrupt rival routes.

Safeguards:

- NPC flow prevents permanent dead Markets;
- high-pop scaling creates more opportunities;
- low-pop scaling preserves solo viability;
- major manipulation creates Events.

---

## 11. Economy-Generated Jobs

Economic state should generate work:

- low Med Gel -> relief Contract;
- high Ore -> freight Contract;
- low Coolant -> emergency supply;
- high hazard -> route-clearing Bounty;
- surplus/shortage pair -> trade opportunity.

M0 has static First Mercy Run. M5 makes job generation broader.

---

## 12. Player Listings

M10+.

Listings should start simple:

- item;
- quantity;
- price;
- seller;
- expiry.

Use for rare salvage, bulk goods, corp supply, asynchronous trade.

---

## 13. Economic UX

Market screens should show:

- buy/sell price;
- stock;
- player quantity;
- capacity;
- demand/surplus note;
- trend or stale recall later.

Avoid formula dumps. Prefer plain reasons.

---

## 14. Recommended First Economy Expansion

After MVP:

- daily restock toward equilibrium;
- one background NPC trade line;
- one shortage-generated Contract;
- one Market digest line;
- one price trend label.

Defer:

- player listings;
- production chains;
- contraband Markets;
- faction pricing;
- large commodity catalog.

---

## 15. Open Questions

1. Should credits be Captain field, ledger, or inventory-like resource?
2. How visible should exact stock be?
3. Should buy and sell use one stock pool or separate pools?
4. Should perishable goods exist?
5. How much manipulation is acceptable on low-pop worlds?
6. Should black-market prices be visible before access?
