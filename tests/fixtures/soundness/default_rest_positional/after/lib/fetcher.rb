class Fetcher
  def pad(text, width = 10, *rest)
    [text, width, rest]
  end

  def go(x)
    pad(x)
  end
end
