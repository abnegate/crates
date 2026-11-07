import { useMemo } from 'react';
import type { Invoice } from './types';

interface InvoiceTableProps {
  invoices: Invoice[];
  onSelect: (id: string) => void;
}

export function InvoiceTable({ invoices, onSelect }: InvoiceTableProps) {
  const sorted = useMemo(
    () => [...invoices].sort((left, right) => right.dueDate.getTime() - left.dueDate.getTime()),
    [invoices],
  );
  return (
    <table>
      <thead>
        <tr>
          <th>Number</th>
          <th>Due</th>
        </tr>
      </thead>
      <tbody>
        {sorted.map((invoice) => (
          <tr key={invoice.id} onClick={() => onSelect(invoice.id)}>
            <td>{invoice.number}</td>
            <td>{invoice.dueDate.toLocaleDateString()}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export const EmptyState = ({ message }: { message: string }) => <p className="empty">{message}</p>;
