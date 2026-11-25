class Csv
  def initialize(rows, separator:)
    @rows = rows
    @separator = separator
  end

  def to_s
    rows = @rows
    header = rows.first.keys.join(",")
    body = rows.map { |row| row.values.join(",") }
    total = rows.sum { |row| row[:amount] }
    lines = [header, *body, "total,#{total}"]
    lines.join(@separator)
  end
end
