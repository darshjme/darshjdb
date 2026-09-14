import { cn } from "../lib/utils";

type BadgeVariant = "default" | "amber" | "emerald" | "red" | "sky" | "purple" | "zinc";

interface BadgeProps {
  children: React.ReactNode;
  variant?: BadgeVariant;
  className?: string;
}

const variants: Record<BadgeVariant, string> = {
  default: "bg-surface-muted text-ink-secondary",
  amber: "bg-brand-500/10 text-brand-500 border border-brand-500/20",
  emerald: "bg-emerald-500/10 text-emerald-700 border border-emerald-500/20",
  red: "bg-red-500/10 text-red-700 border border-red-500/20",
  sky: "bg-sky-500/10 text-sky-700 border border-sky-500/20",
  purple: "bg-purple-500/10 text-purple-700 border border-purple-500/20",
  zinc: "bg-surface-muted/60 text-ink-secondary border border-line-strong/50",
};

export function Badge({ children, variant = "default", className }: BadgeProps) {
  return (
    <span className={cn("badge", variants[variant], className)}>
      {children}
    </span>
  );
}
