//! Selezione del listato in base alla categoria, senza mescolare banche dati.
use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub source_index_url: String,
    pub checked_at: String,
    pub listati: Vec<Listato>,
}
#[derive(Debug, Deserialize)]
pub struct Listato {
    pub nome: String,
    pub patenti: Vec<String>,
    pub procedura: String,
    pub versione: String,
    pub pronto: bool,
    pub fonte: String,
    pub note: String,
    pub exam: Option<VerifiedEdition>,
}
#[derive(Debug, Deserialize)]
pub struct VerifiedEdition {
    pub pdf_sha256: String,
    pub row_count: usize,
    pub digest: String,
    pub version: String,
    pub source: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub question_count: u32,
    pub max_errors: u32,
    pub duration_seconds: u32,
}
pub fn load() -> Result<Catalog> {
    let manifest: Catalog = serde_json::from_str(include_str!("../data/listati.json"))?;
    ensure!(!manifest.listati.is_empty(), "Catalogo dei listati vuoto");
    Ok(manifest)
}
pub fn version_for(license: &str, procedure: &str) -> Result<String> {
    let catalog = load()?;
    let license = license.trim().to_ascii_uppercase();
    let procedure = procedure.trim().to_ascii_lowercase();
    let matches: Vec<&Listato> = catalog
        .listati
        .iter()
        .filter(|x| x.procedura == procedure && x.patenti.iter().any(|c| c == &license))
        .collect();
    ensure!(
        !matches.is_empty(),
        "Nessun listato registrato per patente {} e procedura {}. Esegui 'catalog'.",
        license,
        procedure
    );
    ensure!(
        matches.len() == 1,
        "Catalogo ambiguo per patente {} ({})",
        license,
        procedure
    );
    let item = matches[0];
    ensure!(
        item.pronto,
        "Il listato {} non è abilitato: {}. Fonte: {}",
        item.nome,
        item.note,
        item.fonte
    );
    Ok(item.versione.clone())
}
pub fn print() -> Result<()> {
    let catalog = load()?;
    println!(
        "Fonte indice: {} (controllata: {})",
        catalog.source_index_url, catalog.checked_at
    );
    for item in catalog.listati {
        println!(
            "{} | {} | {} | {} | {}",
            item.patenti.join(", "),
            item.procedura,
            item.nome,
            if item.versione == crate::datasets::LEGACY {
                "AB_<anno>.sqlite3 più recente"
            } else {
                &item.versione
            },
            if item.pronto {
                "selezionabile"
            } else {
                "in preparazione"
            }
        );
        if !item.pronto {
            println!("  Nota: {}", item.note);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ab_shares_the_same_list() {
        assert_eq!(version_for("B", "conseguimento").unwrap(), "AB-7020");
        assert_eq!(version_for("A1", "conseguimento").unwrap(), "AB-7020");
    }
    #[test]
    fn am_is_separate() {
        assert_eq!(version_for("AM", "conseguimento").unwrap(), "AM-MIT-2025");
        assert_ne!(
            version_for("AM", "conseguimento").unwrap(),
            version_for("B", "conseguimento").unwrap()
        );
    }
    #[test]
    fn revision_is_not_conseguimento() {
        assert_ne!(
            version_for("B", "revisione").unwrap(),
            version_for("B", "conseguimento").unwrap()
        );
    }
    #[test]
    fn does_not_serve_unfiltered_superiori() {
        assert!(version_for("C", "conseguimento").is_err());
        assert!(version_for("CQC_MERCI", "cqc").is_err());
    }
    #[test]
    fn invalid_license_is_rejected() {
        assert!(version_for("XX", "conseguimento").is_err());
    }
}
