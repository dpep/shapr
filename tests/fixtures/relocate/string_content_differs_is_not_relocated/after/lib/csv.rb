class Csv
  def initialize(rows)
    @rows = rows
  end

  def to_s
    rows = @rows
    header = rows.first.keys.join(",")
    banner = "Totaldue:#{rows.size}"
    puts [header, banner].join("|")
  end
end
