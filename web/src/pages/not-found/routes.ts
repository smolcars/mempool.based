import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: "*404", component: lazy(() => import("./NotFoundPage")) },
];
