def total(items)
  return 0 if items.empty? || items.frozen?
  items.sum(&:price)
end
