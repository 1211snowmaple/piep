import { Box, Group, Progress, Text, Tooltip } from "@mantine/core";
import { formatDuration, formatNumber, formatRate } from "@/lib/format";

export interface JobProgressProps {
  /** Short description of the step running right now. */
  phase: string;
  processed?: number | null;
  total?: number | null;
  /** Items per second, when the job can measure it. */
  rate?: number | null;
  etaSeconds?: number | null;
  elapsedSeconds?: number | null;
  unit?: string;
  color?: string;
  /** Extra line under the bar, e.g. a count of items that could not be handled. */
  note?: string | null;
  active?: boolean;
}

/**
 * The single progress readout for every long operation in the app.
 *
 * A job that cannot say how much is left still says what it is doing and how
 * long it has been doing it: an indeterminate bar on its own is the thing that
 * makes people wonder whether the app has stopped responding.
 */
export function JobProgress({
  phase,
  processed = null,
  total = null,
  rate = null,
  etaSeconds = null,
  elapsedSeconds = null,
  unit = "件",
  color,
  note = null,
  active = true,
}: JobProgressProps) {
  const determinate = typeof processed === "number" && typeof total === "number" && total > 0;
  const percent = determinate ? Math.min(100, (processed / total) * 100) : 0;

  return (
    <Box role="status" aria-live="polite">
      <Group justify="space-between" wrap="nowrap" gap="xs" mb={6}>
        <Text size="sm" fw={600} className="line-clamp-1">{phase}</Text>
        {determinate && (
          <Text size="sm" c="dimmed" style={{ fontVariantNumeric: "tabular-nums" }}>
            {formatNumber(processed)} / {formatNumber(total)}{unit}（{percent.toFixed(percent < 10 ? 1 : 0)}%）
          </Text>
        )}
      </Group>
      <Tooltip label={determinate ? `${percent.toFixed(1)}%` : "残り時間を計測しています"} disabled={!active}>
        <Progress
          value={determinate ? percent : 100}
          animated={active}
          striped={active && !determinate}
          color={color}
          aria-label={phase}
          {...(determinate
            ? { "aria-valuenow": Math.round(percent), "aria-valuemin": 0, "aria-valuemax": 100 }
            : {})}
        />
      </Tooltip>
      <Group justify="space-between" wrap="nowrap" gap="xs" mt={6}>
        <Text size="xs" c="dimmed">
          {rate ? `${formatRate(rate, unit)}` : elapsedSeconds ? `経過 ${formatDuration(elapsedSeconds)}` : "計測中…"}
        </Text>
        <Text size="xs" c="dimmed">
          {etaSeconds !== null && etaSeconds !== undefined && etaSeconds > 0
            ? `残り約 ${formatDuration(etaSeconds)}`
            : elapsedSeconds
              ? `経過 ${formatDuration(elapsedSeconds)}`
              : ""}
        </Text>
      </Group>
      {note && <Text size="xs" c="dimmed" mt={4}>{note}</Text>}
    </Box>
  );
}
