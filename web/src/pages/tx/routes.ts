import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: "/tx/:txid", component: lazy(() => import("./TransactionPage")) },
];
