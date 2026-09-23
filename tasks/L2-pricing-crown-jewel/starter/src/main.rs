//! `quotes <customer_tiers.csv> <order.txt>`: prints the invoice for an order.

use quotes::customers::CustomerBook;
use quotes::invoice::{build_invoice, render, Order};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: quotes <customer_tiers.csv> <order.txt>");
        std::process::exit(2);
    }
    let run = || -> Result<String, String> {
        let book_text = std::fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
        let order_text = std::fs::read_to_string(&args[2]).map_err(|e| e.to_string())?;
        let book = CustomerBook::parse(&book_text).map_err(|e| e.to_string())?;
        let order = Order::parse(&order_text).map_err(|e| e.to_string())?;
        let invoice = build_invoice(&order, &book).map_err(|e| e.to_string())?;
        Ok(render(&invoice))
    };
    match run() {
        Ok(text) => print!("{text}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
