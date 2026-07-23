import { useState } from "react";

const MAX_POINTS = 150;

export interface TimeSeriesPoint {
  t: number;
  v: number;
}

interface HistoryState {
  points: TimeSeriesPoint[];
  lastUpdatedAt: number;
}

/** Keyed on `updatedAt` rather than `value` so a repeated reading still advances. */
export function useTimeSeries(value: number | undefined, updatedAt: number): TimeSeriesPoint[] {
  const [history, setHistory] = useState<HistoryState>({ points: [], lastUpdatedAt: 0 });

  if (value !== undefined && updatedAt !== 0 && updatedAt !== history.lastUpdatedAt) {
    setHistory({
      points: [...history.points, { t: updatedAt, v: value }].slice(-MAX_POINTS),
      lastUpdatedAt: updatedAt,
    });
  }

  return history.points;
}
