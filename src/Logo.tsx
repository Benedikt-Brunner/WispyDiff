import markSvg from "../assets/logo/mark.svg?raw";
import wordmarkSvg from "../assets/logo/wordmark.svg?raw";

// Inlined (not <img>) so the logo takes the theme's text color via currentColor.
export const Mark = ({ className }: { className?: string }) => (
  <span className={`logo ${className ?? ""}`} aria-hidden dangerouslySetInnerHTML={{ __html: markSvg }} />
);

export const Wordmark = ({ className }: { className?: string }) => (
  <span className={`logo ${className ?? ""}`} role="img" aria-label="WispyDiff" dangerouslySetInnerHTML={{ __html: wordmarkSvg }} />
);
