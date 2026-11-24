class Fetcher
  def join(parts, sep: ",")
    sep << ""
    parts.join(sep)
  end
end
