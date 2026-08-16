import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const MarketsScreen = React.lazy(async () => {
  const module = await import("@/features/markets/ui/MarketsScreen");
  return { default: module.MarketsScreen };
});

export const Route = createFileRoute("/markets")({
  component: MarketsRouteComponent,
});

function MarketsRouteComponent() {
  return (
    <React.Suspense
      fallback={<ViewLoadingFallback includeHeader kind="projects" />}
    >
      <MarketsScreen />
    </React.Suspense>
  );
}
