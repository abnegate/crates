import pytest
import unittest

from widgets.parser import parse_quantity


class TestParseQuantity(unittest.TestCase):
    def test_reads_integers(self):
        self.assertEqual(parse_quantity("4"), 4)

    def test_rejects_negative(self):
        with self.assertRaises(ValueError):
            parse_quantity("-1")


@pytest.mark.parametrize("raw,expected", [("1", 1), ("10", 10)])
def test_parametrized(raw, expected):
    assert parse_quantity(raw) == expected
