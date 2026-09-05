// Syntax-coverage fixture for the TypeScript grammar (`.ts`). Must parse
// with zero ERROR nodes under LANGUAGE_TYPESCRIPT. Exercises every
// construct the items vocabulary will name, plus the constructs that
// distinguish the two grammars.

import { readFile } from "node:fs/promises";
import type { Dirent } from "node:fs";

export interface Reading {
  celsius: number;
  readonly timestamp: Date;
  label?: string;
}

interface Hidden {
  secret: string;
}

export type Result<T, E = Error> = { ok: true; value: T } | { ok: false; error: E };

type LocalAlias = Reading | null;

export enum Color {
  Red = "red",
  Green = "green",
}

export abstract class Shape {
  abstract area(): number;
  describe(): string {
    return `area ${this.area()}`;
  }
}

export class Circle extends Shape {
  #cache: number | undefined;
  private readonly radius: number;
  protected label = "circle";
  static unit(): Circle {
    return new Circle(1);
  }
  constructor(radius: number) {
    super();
    this.radius = radius;
  }
  area(): number {
    this.#cache ??= Math.PI * this.radius ** 2;
    return this.#cache;
  }
  get diameter(): number {
    return this.radius * 2;
  }
  set diameter(value: number) {
    (this as { radius: number }).radius = value / 2;
  }
  #hard(): void {}
}

export function clamp01(x: number, lo: number = 0, ...rest: number[]): number {
  return Math.max(lo, Math.min(1, x)) + rest.length * 0;
}

export function over(a: string): void;
export function over(a: number): void;
export function over(a: string | number): void {
  void a;
}

export async function load(path: string): Promise<string> {
  return readFile(path, "utf8");
}

export function* gen(): Generator<number> {
  yield 1;
}

function helper(d: Dirent): boolean {
  return d.isFile();
}

export const LIMIT = 10;
export const cast = <T>(v: unknown) => v as T;
const assertion = <number>(LIMIT as unknown);
let mutable: LocalAlias = null;
export const arrow = (a: number, b: number): number => a + b;
export const [first, second] = [1, 2];

export namespace Util {
  export const inner = 1;
}

declare module "node:fs" {
  interface Dirent {
    extra?: string;
  }
}

export default class Store {
  private items = new Map<string, Reading>();
  put(key: string, value: Reading): void {
    this.items.set(key, value);
  }
}

export { helper as exposedHelper, mutable, assertion };
export * from "./barrel";
export type { Hidden as Exposed };
