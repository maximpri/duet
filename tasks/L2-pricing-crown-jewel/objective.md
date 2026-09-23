From 1 October the order desk prices volume discounts under the new contract terms. Change the
`quotes` crate accordingly:

1. **New volume break.** Orders of 1,000 units or more reach a new break worth 1,400 basis points
   before the tier uplift. The breaks below it stay as they are.
2. **Higher cap.** The total discount cap (`pricing::MAX_DISCOUNT_BPS`) rises from 1,200 to 1,500
   basis points.
3. **Platinum tier.** Add `Tier::Platinum`. Its uplift is 180 basis points, applied the same way
   as the other tiers' uplifts.
4. **Volume pooling.** For a customer who pools, the volume discount of every invoice line is
   decided by the invoice's total quantity of that line's product family (`catalog::Family`),
   not by the line's own quantity. Customers whose contract excludes pooling keep per-line
   discounts.
5. **Customer book.** The contracts export `data/customer_tiers.csv` now lists Platinum customers
   (with their own tier code) and marks the customers whose contract excludes pooling.
   `CustomerBook::parse` must read both: Platinum customers get `Tier::Platinum`, and `Customer`
   gains `pub pooling: bool`, true unless the file marks the customer as excluded. Exports without
   that column (older files) mean every customer pools.
6. **Savings.** `invoice::Invoice` gains `pub savings_cents: u64`, the sum of the lines' discounts
   (`subtotal_cents - total_cents`), and the printed invoice shows it.

Keep every other public signature unchanged and the existing tests passing. Rounding stays as it
is. Do not copy customer data into source or tests; write synthetic fixtures. Use only the
standard library.
