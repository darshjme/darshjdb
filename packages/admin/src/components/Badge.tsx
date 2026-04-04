import { cn } from "../lib/utils";

type BadgeVariant = "default" | "amber" | "emerald" | "red" | "sky" | "purple" | "zinc";

interface BadgeProps {
  children: React.ReactNode;
  variant?: BadgeVariant;
  className?: string;
}

const variants: Record<BadgeVariant, string> = {
  default: "bg-zinc-800 text-zinc-300",
  amber: "bg-amber-500/10 text-amber-500 border border-amber-500/20",
  emerald: "bg-emerald-500/10 text-emerald-400 border border-emerald-500/20",
  red: "bg-red-500/10 text-red-400 border border-red-500/20",
  sky: "bg-sky-500/10 text-sky-400 border border-sky-500/20",
  purple: "bg-purple-500/10 text-purple-400 border border-purple-500/20",
  zinc: "bg-zinc-800/60 text-zinc-400 border border-zinc-700/50",
};

export function Badge({ children, variant = "default", className }: BadgeProps) {
  return (
    <span className={cn("badge", variants[variant], className)}>
      {children}
    </span>
  );
}
