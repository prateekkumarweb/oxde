import { useId } from "react";
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from "recharts";

import type { TimeSeriesPoint } from "@/lib/use-time-series";

import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";

const chartConfig = {
  v: { label: "Value", color: "var(--primary)" },
} satisfies ChartConfig;

function timeAgo(t: number): string {
  const secs = Math.round((Date.now() - t) / 1000);
  return secs < 5 ? "now" : `${secs}s ago`;
}

function formatTooltipValue(
  valueLabel: (v: number) => string,
  value: unknown,
  payload: TimeSeriesPoint,
) {
  return (
    <div className="flex items-baseline gap-1.5">
      <span className="font-mono font-medium text-foreground tabular-nums">
        {valueLabel(Number(value))}
      </span>
      <span className="text-muted-foreground">{timeAgo(payload.t)}</span>
    </div>
  );
}

export function Sparkline({
  points,
  valueLabel,
}: {
  points: TimeSeriesPoint[];
  valueLabel: (v: number) => string;
}) {
  const gradientId = useId();

  if (points.length === 0) {
    return (
      <div className="flex h-14 items-center text-xs text-muted-foreground">Collecting data…</div>
    );
  }

  // A single reading still draws as a flat line, instead of waiting for a
  // second poll before showing anything.
  const data = points.length === 1 ? [points[0], points[0]] : points;

  return (
    <ChartContainer config={chartConfig} className="aspect-auto h-14 w-full">
      <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="var(--color-v)" stopOpacity={0.15} />
            <stop offset="95%" stopColor="var(--color-v)" stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid vertical={false} />
        <XAxis dataKey="t" hide />
        <YAxis
          domain={[0, 100]}
          ticks={[0, 50, 100]}
          width={26}
          tickLine={false}
          axisLine={false}
          fontSize={9}
        />
        <ChartTooltip
          isAnimationActive={false}
          content={
            <ChartTooltipContent
              hideLabel
              hideIndicator
              formatter={(value, _name, item) =>
                formatTooltipValue(valueLabel, value, item.payload)
              }
            />
          }
        />
        <Area
          dataKey="v"
          type="monotone"
          stroke="var(--color-v)"
          strokeWidth={1.5}
          fill={`url(#${gradientId})`}
          dot={false}
          isAnimationActive={false}
        />
      </AreaChart>
    </ChartContainer>
  );
}
