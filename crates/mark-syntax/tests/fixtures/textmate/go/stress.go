package stress

import (
	"fmt"
	"regexp"
)

/* Go stress fixture.
   Multi-line comment with non-ASCII text: café λ🚀.
*/
const raw = `line one
line two with "quotes" and λ🚀
${not_interpolation}`

func Ratio(total, count float64) float64 {
	return total / count
}

func main() {
	re := regexp.MustCompile(`^/api/[\p{L}-]+$`)
	fmt.Println(re.MatchString("/api/café"), Ratio(42, 2), raw)
}
