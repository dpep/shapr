def total(items)
  return 0 if items.empty?
  items.sum(&:price)
end
