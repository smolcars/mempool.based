import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: "/mempool", component: lazy(() => import("./MempoolPage")) },
  // Replacements are part of the mempool's section: what left it for a better offer.
  { path: "/rbf", component: lazy(() => import("./RbfPage")) },
];
