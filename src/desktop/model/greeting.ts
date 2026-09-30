/**
 * What a new session's home says above its composer, by the hour of the
 * person's day: "Working late?" only when it is late, as the night scene
 * suggests, and a plain greeting the rest of the time.
 */
export function greetingAt(hour: number): string {
  if (hour >= 5 && hour < 12) return "Good morning"
  if (hour >= 12 && hour < 17) return "Good afternoon"
  if (hour >= 17 && hour < 22) return "Good evening"
  return "Working late?"
}
