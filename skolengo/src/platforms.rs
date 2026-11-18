// SPDX-License-Identifier: GPL-3.0-or-later

//! The school platforms built on Skolengo, each with its code (the header
//! `X-Skolengo-Ems-Code`) and its OpenID Connect discovery address: the
//! table of scolengo-api's README (Maël Gangloff, GPL-3.0-or-later),
//! `README.md:36-54` at commit 4241a20. The discovery address is also what
//! the platform's school search gives as `emsOIDCWellKnownUrl` (ha-skolengo,
//! `api.py:270-300`). The manifest names each discovery address's host:
//! Sioul refuses any other.

/// A platform: its code, its usual name, its discovery address.
pub struct Platform {
    pub code: &'static str,
    pub name: &'static str,
    pub discovery: &'static str,
}

pub const PLATFORMS: [Platform; 19] = [
    Platform { code: "gdest", name: "Mon Bureau Numérique", discovery: "https://sso.monbureaunumerique.fr/oidc/.well-known" },
    Platform { code: "entmip", name: "Mon ENT Occitanie", discovery: "https://sso.mon-ent-occitanie.fr/oidc/.well-known" },
    Platform { code: "cg76", name: "Arsène 76", discovery: "https://sso.arsene76.fr/oidc/.well-known" },
    Platform { code: "cg27", name: "ENT27", discovery: "https://sso.ent27.fr/oidc/.well-known" },
    Platform { code: "cg23", name: "ENT Creuse", discovery: "https://sso.entcreuse.fr/oidc/.well-known" },
    Platform { code: "rra", name: "ENT Auvergne-Rhône-Alpes", discovery: "https://sso.ent.auvergnerhonealpes.fr/oidc/.well-known" },
    Platform { code: "cg06", name: "Agora 06", discovery: "https://sso.agora06.fr/oidc/.well-known" },
    Platform { code: "cg42", name: "CyberCollèges 42", discovery: "https://sso.cybercolleges42.fr/oidc/.well-known" },
    Platform { code: "cg31", name: "eCollège 31 Haute-Garonne", discovery: "https://sso.ecollege.haute-garonne.fr/oidc/.well-known" },
    Platform { code: "cg95", name: "Mon collège en Val d'Oise", discovery: "https://sso.moncollege.valdoise.fr/oidc/.well-known" },
    Platform { code: "cg93", name: "Webcollège Seine-Saint-Denis", discovery: "https://sso.webcollege.seinesaintdenis.fr/oidc/.well-known" },
    Platform { code: "bfc", name: "Eclat-BFC", discovery: "https://sso.eclat-bfc.fr/oidc/.well-known" },
    Platform { code: "cg84", name: "@ucollège84", discovery: "https://sso.aucollege84.vaucluse.fr/oidc/.well-known" },
    Platform { code: "cg94", name: "ENT Val de Marne", discovery: "https://sso.moncollege.valdemarne.fr/oidc/.well-known" },
    Platform { code: "metab1d", name: "Skolengo-Écoles primaires", discovery: "https://sso.skolengo.com/oidc/.well-known" },
    Platform { code: "mpdl", name: "Skolengo-Collèges et Lycées", discovery: "https://sso.pdl.kosmoseducation.com/oidc/.well-known" },
    Platform { code: "metab", name: "AEFE", discovery: "https://sso1.skolengo.com/oidc/.well-known" },
    Platform { code: "metabam", name: "AEFE Amérique - Pacifique", discovery: "https://sso2.skolengo.com/oidc/.well-known" },
    Platform { code: "cgb", name: "Schulportal Ostbelgien", discovery: "https://sso.schulen.be/oidc/.well-known" },
];

/// The platform of a code.
pub fn by_code(code: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|p| p.code == code)
}
