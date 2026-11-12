import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: ["/blocks", "/blocks/:from"], component: lazy(() => import("./BlocksPage")) },
];
