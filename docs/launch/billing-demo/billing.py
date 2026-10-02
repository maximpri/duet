from decimal import Decimal
import csv
import io

def active_total(text):
    rows = csv.DictReader(io.StringIO(text))
    return sum((Decimal(row["amount"]) for row in rows if "active" in row["status"]), Decimal("0"))
