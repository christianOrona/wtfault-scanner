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
export const REPO_URL: string | null = null;

/**
 * The author's LinkedIn profile.
 *
 * Example: "https://www.linkedin.com/in/your-name"
 */
export const LINKEDIN_URL: string | null = null;
