import type { ComponentProps } from "react";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function LibraryWorkCard({ className, ...props }: ComponentProps<typeof Card>) {
  return <Card className={cn("download-card", className)} {...props} />;
}
