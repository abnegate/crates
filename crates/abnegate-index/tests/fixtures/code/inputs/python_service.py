"""Order intake service.

Validates incoming orders and hands them to the fulfilment queue.
"""

import json
import logging
from dataclasses import dataclass, field
from typing import Iterable, Optional

from .queue import FulfilmentQueue

LOGGER = logging.getLogger(__name__)
MAX_LINES = 50


@dataclass
class OrderLine:
    sku: str
    quantity: int
    unit_price: float = 0.0


@dataclass
class Order:
    reference: str
    lines: list = field(default_factory=list)
    note: Optional[str] = None

    def total(self) -> float:
        """Sum of every line's price times quantity."""
        return sum(line.quantity * line.unit_price for line in self.lines)


class OrderIntake:
    """Accept orders, validate them, and queue them for fulfilment."""

    def __init__(self, queue: FulfilmentQueue, strict: bool = True):
        self.queue = queue
        self.strict = strict
        self.rejected = 0

    def validate(self, order: Order) -> list:
        problems = []
        if not order.reference:
            problems.append("missing reference")
        if len(order.lines) > MAX_LINES:
            problems.append(f"too many lines: {len(order.lines)}")
        for line in order.lines:
            if line.quantity <= 0:
                problems.append(f"bad quantity for {line.sku}")
        return problems

    def submit(self, order: Order) -> bool:
        problems = self.validate(order)
        if problems and self.strict:
            self.rejected += 1
            LOGGER.warning("rejected %s: %s", order.reference, problems)
            return False
        self.queue.push(json.dumps({"reference": order.reference, "total": order.total()}))
        return True


def parse_lines(rows: Iterable[dict]) -> list:
    return [OrderLine(row["sku"], int(row["quantity"]), float(row.get("price", 0))) for row in rows]


def test_parse_lines_reads_price():
    lines = parse_lines([{"sku": "A1", "quantity": "2", "price": "1.5"}])
    assert lines[0].unit_price == 1.5
