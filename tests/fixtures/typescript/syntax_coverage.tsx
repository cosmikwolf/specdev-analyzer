// Syntax-coverage fixture for the TSX grammar (`.tsx`). Must parse with
// zero ERROR nodes under LANGUAGE_TSX. JSX plus the same declaration
// kinds; no `<T>x` assertions (the TSX grammar cannot parse them).

import type { ReactNode } from "react";

export interface CardProps {
  title: string;
  children?: ReactNode;
}

export type Variant = "primary" | "ghost";

export enum Size {
  Small,
  Large,
}

export function EventCard({ title, children }: CardProps): ReactNode {
  return (
    <section className="card">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

export const Badge = ({ label }: { label: string }) => <span>{label}</span>;

export class Counter {
  #count = 0;
  private step: number;
  constructor(step: number = 1) {
    this.step = step;
  }
  next(): number {
    this.#count += this.step;
    return this.#count;
  }
  render(): ReactNode {
    return <em>{this.next()}</em>;
  }
}

export default function Page(): ReactNode {
  const generic = (v: unknown) => v as string;
  return <EventCard title={generic("x")}><Badge label="new" /></EventCard>;
}

export * from "./widgets";
