package main

import (
	"fmt"
	"os"
	"sort"
	"text/tabwriter"
	"time"
)

type Attestation struct {
	ID        string    `json:"id"`
	Status    string    `json:"status"`
	CreatedAt time.Time `json:"created_at"`
	ExpiresAt time.Time `json:"expires_at"`
	Subject   string    `json:"subject"`
	Method    string    `json:"method"`
}

func ShowAttestations(atts []Attestation) {
	if len(atts) == 0 {
		fmt.Println("No attestations found.")
		return
	}

	sort.Slice(atts, func(i, j int) bool {
		return atts[i].CreatedAt.After(atts[j].CreatedAt)
	})

	w := tabwriter.NewWriter(os.Stdout, 0, 0, 3, ' ', 0)
	fmt.Fprintln(w, "ID\tSTATUS\tMETHOD\tSUBJECT\tCREATED\tEXPIRES")
	for _, a := range atts {
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%s\t%s\n",
			a.ID[:8], a.Status, a.Method, truncate(a.Subject, 30),
			a.CreatedAt.Format("2006-01-02 15:04"),
			a.ExpiresAt.Format("2006-01-02"))
	}
	w.Flush()
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-3] + "..."
}
