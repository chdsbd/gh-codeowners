// For each case in a fixture, prints hmarr/codeowners' verdict: 1 if the
// line owns the file, 0 if not, E if the line is rejected, P if the library
// panics.
package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"github.com/hmarr/codeowners"
)

func verdict(line, path string) (result string) {
	defer func() {
		if recover() != nil {
			result = "P"
		}
	}()
	rules, err := codeowners.ParseFile(strings.NewReader(line + "\n"))
	if err != nil || len(rules) == 0 {
		return "E"
	}
	rule, err := rules.Match(path)
	if err != nil {
		return "E"
	}
	if rule != nil {
		return "1"
	}
	return "0"
}

func main() {
	f, err := os.Open(os.Args[1])
	if err != nil {
		panic(err)
	}
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		text := scanner.Text()
		if text == "" || strings.HasPrefix(text, "#") {
			continue
		}
		fields := strings.Split(text, " | ")
		fmt.Println(verdict(fields[0], fields[1]))
	}
}
