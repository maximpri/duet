import unittest
from decimal import Decimal
from billing import active_total

class BillingTests(unittest.TestCase):
    def test_excludes_inactive(self):
        self.assertEqual(active_total("status,amount\nactive,10.25\ninactive,99.00\n"), Decimal("10.25"))
    def test_exact_status(self):
        self.assertEqual(active_total("status,amount\nreactivated,50\nactive,7\n"), Decimal("7"))
    def test_empty_export(self):
        self.assertEqual(active_total("status,amount\n"), Decimal("0"))
    def test_decimal_precision(self):
        self.assertEqual(active_total("status,amount\nactive,0.10\nactive,0.20\n"), Decimal("0.30"))

if __name__ == "__main__":
    unittest.main()
