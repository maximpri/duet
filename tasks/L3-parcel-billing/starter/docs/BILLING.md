# Billing rules

parcelflow resells parcel delivery from 30 carriers. Every month we invoice each customer for
the shipments delivered to its recipients that month, priced with our own tariff.

## What is billed

- A shipment is billed in the month of its **delivery** (final status `delivered`), as a UTC
  calendar month. Carriers report the time of the latest status; that is the delivery time.
- Shipments in any other status are not billed that month.
- A shipment whose customer id is not in `data/customers.csv` is skipped and reported.

## Price of one shipment

1. **Billable weight** = the larger of the actual weight and the dimensional weight, rounded up
   to the next 500 g. Dimensional weight in kg = length × width × height in cm ÷ the carrier's
   volumetric divisor (`config/carriers*.conf`), i.e. mm³ ÷ divisor in grams, rounded up.
2. A shipment whose billable weight exceeds its service limit (express 30 kg, standard 31.5 kg,
   economy 40 kg, freight 1000 kg) is rejected and reported, not billed.
3. **Base price** from `config/rates.csv` by service and zone: the first 500 g plus every further
   500 g. Zones: 1 Germany, 2 neighbours, 3 rest of the EU, 4 rest of Europe, 5 world. No rate
   for a service and zone means the shipment is rejected.
4. **Surcharges** (carrier settings):
   - fuel: `fuel_pct` percent of the base price;
   - remote area: `remote_fee` when the destination postal code lies in a range of
     `config/remote_areas.csv` for its country (both range ends included);
   - oversize: `oversize_fee` when the longest side exceeds 120 cm or length plus girth exceeds
     300 cm.
5. **Contract discount**: the customer's `discount_pct` applies to the **base price only**.
   Surcharges are pass-through costs and are never discounted.
6. Total = base − discount + fuel + remote + oversize.

Every percentage is rounded half up to the cent on its own. Amounts are in EUR.

## Configuration

`.env` selects the inputs: `CARRIER_CONFIG` (default `config/carriers.conf`), `RATES_FILE`,
`REMOTE_AREAS_FILE`, `CUSTOMERS_FILE`, `FEEDS_DIR`. Relative paths are resolved against the
project root.
