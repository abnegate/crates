import { HttpClient } from './http-client';
import type { Invoice, InvoiceLine } from './types';

export enum InvoiceState {
  Draft = 'draft',
  Sent = 'sent',
  Paid = 'paid',
}

export interface InvoiceStore {
  find(id: string): Promise<Invoice | undefined>;
  save(invoice: Invoice): Promise<void>;
}

export type LineTotal = (line: InvoiceLine) => number;

const lineTotal: LineTotal = (line) => line.quantity * line.unitPrice;

export class InvoiceService {
  private readonly client: HttpClient;
  private readonly store: InvoiceStore;

  constructor(client: HttpClient, store: InvoiceStore) {
    this.client = client;
    this.store = store;
  }

  async total(id: string): Promise<number> {
    const invoice = await this.store.find(id);
    if (!invoice) {
      throw new Error(`invoice ${id} not found`);
    }
    return invoice.lines.map(lineTotal).reduce((sum, value) => sum + value, 0);
  }

  async send(id: string, recipient: string): Promise<InvoiceState> {
    const invoice = await this.store.find(id);
    if (!invoice || invoice.state !== InvoiceState.Draft) {
      return invoice?.state ?? InvoiceState.Draft;
    }
    await this.client.post('/api/invoices/send', { id, recipient });
    invoice.state = InvoiceState.Sent;
    await this.store.save(invoice);
    return invoice.state;
  }
}

export function isOverdue(invoice: Invoice, now: Date = new Date()): boolean {
  return invoice.state !== InvoiceState.Paid && invoice.dueDate.getTime() < now.getTime();
}
