// shortcut: placeholder for pages not built yet; each page task deletes its use, the last one deletes this file.
import { SectionHeader } from "../components/SectionHeader";
import { usePageTitle } from "../lib/page";
import "./Stub.css";

/** A clearly marked empty page: which page it is, what it will show and which task builds it. */
export function Stub(props: { title: string; task: string; shows: string; subject?: string }) {
  usePageTitle(() => props.title);
  return (
    <div class="wrap stub">
      <h1>{props.title}</h1>
      {props.subject && <p class="stub__subject">{props.subject}</p>}
      <SectionHeader title="Not built yet" />
      <p class="stub__text">
        This page will show {props.shows}. It is being built in task {props.task}.
      </p>
    </div>
  );
}
