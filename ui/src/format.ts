//! Turning stored values into the strings a row shows.
//!
//! Two rules run through all of it. Missing is not zero: a call the OTLP lane
//! never saw has no duration and no cost, and rendering either as `0` would
//! claim a measurement that was never made (task 5.5). And a number a person
//! compares against its neighbours is tabular — the CSS supplies the figures,
//! this supplies the same number of them.

const TIME = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});

const DAY = new Intl.DateTimeFormat(undefined, {
  weekday: "long",
  day: "numeric",
  month: "long",
});

const DAY_YEAR = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "long",
  year: "numeric",
});

const FULL = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "medium",
});

export const EM_DASH = "—";

export function clock(ms: number | null): string {
  return ms === null ? EM_DASH : TIME.format(ms);
}

export function fullStamp(ms: number | null): string {
  return ms === null ? EM_DASH : FULL.format(ms);
}

/** "Today", "Yesterday", or a written date — the sticky heading over the list. */
export function dayLabel(ms: number): string {
  const day = new Date(ms);
  const today = new Date();
  const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
  if (same(day, today)) return `Today · ${DAY.format(ms)}`;
  const yesterday = new Date(today.getTime() - 86_400_000);
  if (same(day, yesterday)) return `Yesterday · ${DAY.format(ms)}`;
  return day.getFullYear() === today.getFullYear() ? DAY.format(ms) : DAY_YEAR.format(ms);
}

/**
 * A duration, at the precision worth reading.
 *
 * Sub-second work is measured in milliseconds and anything longer in seconds:
 * "1240 ms" and "0.04 s" are both true and neither is the thing you wanted.
 */
export function duration(ms: number | null): string {
  if (ms === null) return EM_DASH;
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${String(seconds).padStart(2, "0")}s`;
}

export function count(n: number): string {
  return n.toLocaleString();
}

/** A big number at tile size: 1,284 / 12.9K / 4.2M. */
export function compact(n: number): string {
  return n < 10_000
    ? n.toLocaleString()
    : new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

/**
 * A stretch of working time, at the precision a day of work is read in.
 *
 * [`duration`] is for one call and tops out at minutes; this is for the sum of
 * a week's, where "512m 30s" is a number nobody converts in their head.
 */
export function elapsed(ms: number): string {
  if (ms <= 0) return "none";
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h ${rest}m`;
}

/** A fraction as a percentage, or a dash when nothing was measured. */
export function percent(ratio: number | null, digits = 1): string {
  return ratio === null ? EM_DASH : `${(ratio * 100).toFixed(digits)}%`;
}

/**
 * A cost in micro-dollars.
 *
 * `null` means the OTLP lane never saw this session, which is the ordinary
 * case for imported history — so it renders as "no cost recorded", never as
 * free.
 */
export function cost(micros: number | null): string {
  if (micros === null) return EM_DASH;
  const dollars = micros / 1_000_000;
  if (dollars === 0) return "$0.00";
  return dollars < 0.01 ? "<$0.01" : `$${dollars.toFixed(2)}`;
}

export function bytes(n: number | null): string {
  if (n === null) return EM_DASH;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

/** The last segment of a path — what a project or a file is called. */
export function basename(path: string | null): string {
  if (!path) return EM_DASH;
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

/**
 * A file path shortened from the left, keeping the end.
 *
 * The informative half of a path is the file, not the home directory it is
 * eventually under.
 */
export function shortPath(path: string, keep = 3): string {
  const parts = path.split("/").filter(Boolean);
  if (parts.length <= keep) return path;
  return `…/${parts.slice(-keep).join("/")}`;
}

/** The lanes that witnessed a call, in the words the export uses. */
export function lanes(provenance: number): string {
  const transcript = (provenance & 1) !== 0;
  const otel = (provenance & 2) !== 0;
  if (transcript && otel) return "both lanes";
  if (transcript) return "transcript only";
  if (otel) return "OTEL only";
  return "neither lane";
}
