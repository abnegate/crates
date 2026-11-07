import React, { useState, useEffect } from 'react';
import PropTypes from 'prop-types';

export function CartBadge({ count, onOpen }) {
  const [pulse, setPulse] = useState(false);

  useEffect(() => {
    if (count === 0) {
      return undefined;
    }
    setPulse(true);
    const timer = setTimeout(() => setPulse(false), 300);
    return () => clearTimeout(timer);
  }, [count]);

  return (
    <button className={pulse ? 'badge badge--pulse' : 'badge'} onClick={onOpen}>
      <span aria-label="items in cart">{count}</span>
    </button>
  );
}

CartBadge.propTypes = {
  count: PropTypes.number.isRequired,
  onOpen: PropTypes.func,
};

export default function CartSummary({ items }) {
  const total = items.reduce((sum, item) => sum + item.price * item.quantity, 0);
  return (
    <section>
      <h2>Cart</h2>
      <ul>
        {items.map((item) => (
          <li key={item.id}>{item.name} x {item.quantity}</li>
        ))}
      </ul>
      <p>Total: {total.toFixed(2)}</p>
    </section>
  );
}
