class Fetcher
  def join(parts, sep: ",")
    parts.join(sep)
  end

  def go(x)
    join(x, sep: ',')
  end
end
