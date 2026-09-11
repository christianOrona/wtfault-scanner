// Everything about the product that is a name or a link rather than behaviour.
//
// One file, so changing where the project lives is a one-line edit rather than
// a search through components. Anything unset is rendered as "not set yet" by
// the About screen instead of appearing as a broken link — a dead link in an
// About box is worse than an obvious gap.

export const PRODUCT_NAME = "WTFault Scanner";

export const TAGLINE = "Just ask your car what the fuck is wrong.";

/**
 * Where the source lives. Set this once the repository exists.
 *
 * Example: "https://github.com/your-name/wtfault-scanner"
 */
export const REPO_URL: string | null =
  "https://github.com/christianOrona/wtfault-scanner";

/**
 * The author's LinkedIn profile.
 *
 * Example: "https://www.linkedin.com/in/your-name"
 */
export const LINKEDIN_URL: string | null =
  "https://www.linkedin.com/in/christian-orona-30957335/";

/** How the project is licensed, shown wherever the name is. */
export const LICENCE = "MIT OR Apache-2.0";

/** One place a person can go, and what they will find there. */
export interface ProjectLink {
  label: string;
  href: string;
}

/**
 * The links shown on the splash and the About screen.
 *
 * Derived from the two constants above rather than typed out again, so the
 * project moving is still the one-line edit this file promises. A null URL
 * drops out of the list instead of rendering as a dead link — an About box
 * full of broken links is worse than a short one.
 *
 * Add a link by adding a line. Nothing here is fetched by the app; every one
 * of these opens in the person's own browser when they click it.
 */
export const LINKS: ProjectLink[] = [
  REPO_URL && { label: "Source code", href: REPO_URL },
  REPO_URL && { label: "Report a bug", href: `${REPO_URL}/issues` },
  REPO_URL && { label: "Releases", href: `${REPO_URL}/releases` },
  LINKEDIN_URL && { label: "The author", href: LINKEDIN_URL },
].filter((l): l is ProjectLink => !!l);
