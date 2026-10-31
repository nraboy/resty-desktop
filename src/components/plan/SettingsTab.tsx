import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { SETTINGS_SECTIONS, SETTINGS_SECTION_LABELS, sectionIsCustom, type SettingsSection } from "../../lib/planForm";
import { activeSectionIndex } from "../../lib/scrollSpy";
import BandwidthCard from "./BandwidthCard";
import ResticOptionsCard from "./ResticOptionsCard";
import RetentionSection from "./RetentionSection";
import SettingsNav from "./SettingsNav";
import TagsSection from "./TagsSection";
import WebhooksCard from "./WebhooksCard";
import type { FormProps } from "./types";

interface Props extends FormProps {
  /** A new plan whose retention was pre-filled from Settings' default policy. */
  seededFromDefault: boolean;
  /** The section the page asks to show (from the URL); a new value scrolls there. */
  requested: SettingsSection | null;
  /** Bumped by the page to ask for `requested` again when it hasn't changed (a failed save in
   * the section the URL already names, after the user scrolled away). */
  requestKey?: number;
  /** A name in the jump list was clicked (the pane has already scrolled). */
  onJump: (s: SettingsSection) => void;
  /** The section where the last save failed, if any. */
  alertSection: SettingsSection | null;
}

const sectionId = (s: SettingsSection) => `plan-section-${s}`;

/** Room left above a section's heading when the pane jumps to it. */
const JUMP_GAP = 18;
/** The least empty space below the last section. More is added when needed so that the last
 * section can always be scrolled to the top (otherwise jumping to it, or to the one before it,
 * would stop short at the bottom). */
const MIN_TAIL = 220;
/** How long the pane must be still before a jump that didn't arrive is given up. */
const SETTLE_MS = 150;

const prefersReducedMotion = () =>
  typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/**
 * The Settings tab: one scrolling pane holding every section (Retention, Tags, Bandwidth, Restic
 * options, Webhooks), with a quiet jump list beside it. Clicking a name scrolls the pane to that
 * section; scrolling moves the highlight (scroll-spy, lib/scrollSpy.ts).
 */
export default function SettingsTab({
  form,
  update,
  seededFromDefault,
  requested,
  requestKey = 0,
  onJump,
  alertSection,
}: Props) {
  const paneRef = useRef<HTMLDivElement>(null);
  const tailRef = useRef<HTMLDivElement>(null);
  const sectionEls = useRef<Partial<Record<SettingsSection, HTMLElement | null>>>({});
  const [active, setActive] = useState<SettingsSection>(requested ?? SETTINGS_SECTIONS[0]);

  // While a jump's smooth scroll is under way, the scrollTop it is heading for. The highlight
  // stays on the jumped-to section until the pane gets there (or stops, or the user takes over)
  // instead of sweeping through every section in between.
  const jumpTarget = useRef<number | null>(null);
  const settleTimer = useRef<number | undefined>(undefined);
  const frame = useRef(0);

  const spy = useCallback(() => {
    const pane = paneRef.current;
    if (!pane || !pane.clientHeight) return; // hidden (Files tab showing)
    const tops = SETTINGS_SECTIONS.map((s) => sectionEls.current[s]?.offsetTop ?? 0);
    setActive(SETTINGS_SECTIONS[activeSectionIndex(tops, pane.scrollTop, pane.clientHeight, pane.scrollHeight)]);
  }, []);

  // Sizes the empty space below the last section so it can reach the top of the pane. Set on
  // the DOM directly so a jump right after (the first request on mount) sees the final height.
  const fitTail = useCallback(() => {
    const pane = paneRef.current;
    const tail = tailRef.current;
    const last = sectionEls.current[SETTINGS_SECTIONS[SETTINGS_SECTIONS.length - 1]];
    if (!pane || !tail || !last || !pane.clientHeight) return;
    tail.style.height = `${Math.max(MIN_TAIL, pane.clientHeight - last.offsetHeight - JUMP_GAP)}px`;
  }, []);

  const endJump = useCallback(() => {
    window.clearTimeout(settleTimer.current);
    jumpTarget.current = null;
  }, []);

  const scrollToSection = useCallback(
    (s: SettingsSection, smooth: boolean) => {
      const pane = paneRef.current;
      const el = sectionEls.current[s];
      if (!pane || !el) return;
      fitTail();
      const top = Math.max(0, Math.min(el.offsetTop - JUMP_GAP, pane.scrollHeight - pane.clientHeight));
      setActive(s);
      endJump();
      if (Math.abs(pane.scrollTop - top) < 1) return;
      jumpTarget.current = top;
      // A scroll that never starts (or never reports) must not hold the highlight forever.
      settleTimer.current = window.setTimeout(() => {
        jumpTarget.current = null;
        spy();
      }, 1000);
      pane.scrollTo({ top, behavior: smooth && !prefersReducedMotion() ? "smooth" : "auto" });
    },
    [endJump, fitTail, spy],
  );

  const onScroll = () => {
    const pane = paneRef.current;
    if (!pane) return;
    if (jumpTarget.current !== null) {
      window.clearTimeout(settleTimer.current);
      if (Math.abs(pane.scrollTop - jumpTarget.current) > 1) {
        // Still on the way. If it stops short, hand back to the spy once the pane is still.
        settleTimer.current = window.setTimeout(() => {
          jumpTarget.current = null;
          spy();
        }, SETTLE_MS);
        return;
      }
      jumpTarget.current = null;
    }
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      spy();
    });
  };

  // The page's requests (the URL's section, a failed save). Only a new section, or the same one
  // asked for again with a new key, scrolls; the first one, on opening the tab, lands instantly.
  // A null request (the Files tab is showing) is ignored, so coming back doesn't re-scroll.
  const lastRequest = useRef<{ section: SettingsSection; key: number } | null>(null);
  const mounted = useRef(false);
  useLayoutEffect(() => {
    if (!requested) return;
    const prev = lastRequest.current;
    if (prev && prev.section === requested && prev.key === requestKey) return;
    lastRequest.current = { section: requested, key: requestKey };
    scrollToSection(requested, mounted.current);
  }, [requested, requestKey, scrollToSection]);
  useEffect(() => {
    mounted.current = true;
  }, []);

  // Refit the tail and re-read the highlight when the pane or the last section changes size —
  // including when the tab is shown again after the Files tab (its size goes from 0 back).
  useEffect(() => {
    const pane = paneRef.current;
    const last = sectionEls.current[SETTINGS_SECTIONS[SETTINGS_SECTIONS.length - 1]];
    if (!pane || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      fitTail();
      if (jumpTarget.current === null) spy();
    });
    ro.observe(pane);
    if (last) ro.observe(last);
    return () => ro.disconnect();
  }, [fitTail, spy]);

  useEffect(
    () => () => {
      window.clearTimeout(settleTimer.current);
      cancelAnimationFrame(frame.current);
    },
    [],
  );

  const jump = (s: SettingsSection) => {
    // The page writes this section to the URL, which comes back as `requested`: already handled.
    lastRequest.current = { section: s, key: requestKey };
    scrollToSection(s, true);
    onJump(s);
  };

  const body: Record<SettingsSection, ReactNode> = {
    retention: <RetentionSection form={form} update={update} seededFromDefault={seededFromDefault} />,
    tags: <TagsSection form={form} update={update} />,
    bandwidth: <BandwidthCard form={form} update={update} />,
    restic: <ResticOptionsCard form={form} update={update} />,
    webhooks: <WebhooksCard form={form} update={update} />,
  };

  return (
    <div className="flex h-full min-h-0">
      <SettingsNav active={active} custom={(s) => sectionIsCustom(form, s)} alert={alertSection} onJump={jump} />
      <div
        ref={paneRef}
        onScroll={onScroll}
        // The user taking over mid-jump hands the highlight straight back to the spy.
        onWheel={endJump}
        onTouchStart={endJump}
        onPointerDown={endJump}
        onKeyDown={endJump}
        className="relative flex-1 min-w-0 overflow-y-auto pt-4 pl-3 pr-6"
      >
        {SETTINGS_SECTIONS.map((s) => (
          <section
            key={s}
            id={sectionId(s)}
            aria-labelledby={`${sectionId(s)}-title`}
            ref={(el) => {
              sectionEls.current[s] = el;
            }}
            className="mb-6"
          >
            <h2 id={`${sectionId(s)}-title`} className="text-base font-semibold text-gray-100 mb-2">
              {SETTINGS_SECTION_LABELS[s]}
            </h2>
            {body[s]}
          </section>
        ))}
        <div ref={tailRef} aria-hidden="true" className="h-[220px]" />
      </div>
    </div>
  );
}
